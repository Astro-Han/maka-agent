/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

mod profile;
mod view;
use super::model_inventory as catalog;
use super::{Command as Manage, Entity, Target};
use crate::{
    app::{Action, App},
    editor::Editor,
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use maka_protocol::configuration::ConnectionCatalogQueryInput;
use serde_json::Value;
pub(super) use view::draw;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Search,
    Toggle(String),
    Retry,
    Profile(profile::Command),
}
impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Search => "enabled-model-search",
            Self::Toggle(_) => "enabled-model-toggle",
            Self::Retry => "command-refresh",
            Self::Profile(command) => command.label(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    generation: u64,
    target: Target,
    pub query: ConnectionCatalogQueryInput,
}
pub(super) struct State {
    generation: u64,
    catalog: catalog::Catalog,
    pub selected: Vec<String>,
    search: Editor,
    row: usize,
    focus: usize, // search, list, cancel, save: stable across all loading states.
    hovered: Option<Manage>,
    pub edit_profiles: bool,
    profile: Option<profile::Draft>,
}
impl State {
    pub fn new(
        generation: u64,
        target: &Target,
        query: ConnectionCatalogQueryInput,
        edit_profiles: bool,
    ) -> Self {
        let Entity::Connection(row) = &target.entity else {
            unreachable!()
        };
        let catalog = catalog::Catalog::new(row.clone(), query);
        Self {
            generation,
            catalog: if edit_profiles {
                catalog.with_overrides()
            } else {
                catalog
            },
            selected: row.model_ids.clone(),
            search: Editor::bounded(128, "enabled-model-search-limit"),
            row: 0,
            focus: 0,
            hovered: None,
            edit_profiles,
            profile: None,
        }
    }
    pub fn can_save(&self) -> bool {
        if self.edit_profiles {
            return self.catalog.ready
                && self.catalog.error.is_none()
                && self.profile.as_ref().is_some_and(|draft| {
                    draft.value().is_ok_and(|value| {
                        self.catalog
                            .overrides
                            .get(&draft.id)
                            .cloned()
                            .unwrap_or_default()
                            != value
                            && (self.catalog.overrides.contains_key(&draft.id)
                                || self.catalog.overrides.len() < 2048)
                    })
                });
        }
        self.catalog.ready
            && self.catalog.error.is_none()
            && (self.selected.len() != self.catalog.basis.model_ids.len()
                || self
                    .selected
                    .iter()
                    .any(|id| !self.catalog.basis.model_ids.contains(id)))
    }
    pub fn updated_profiles(
        &self,
    ) -> Option<std::collections::BTreeMap<String, maka_protocol::configuration::ModelOverride>>
    {
        let draft = self.profile.as_ref()?;
        let mut updated = self.catalog.overrides.clone();
        let value = draft.value().ok()?;
        if value == maka_protocol::configuration::ModelOverride::default() {
            updated.remove(&draft.id);
        } else {
            updated.insert(draft.id.clone(), value);
        }
        maka_protocol::configuration::validation::profiles(&updated).ok()?;
        Some(updated)
    }
    pub fn selected_ids(&self) -> Vec<String> {
        self.catalog
            .basis
            .model_ids
            .iter()
            .filter(|id| self.selected.contains(id))
            .chain(
                self.selected
                    .iter()
                    .filter(|id| !self.catalog.basis.model_ids.contains(id)),
            )
            .cloned()
            .collect()
    }
    pub fn invalidate_geometry(&mut self) {
        self.search.invalidate_geometry();
        self.hovered = None;
        if let Some(profile) = &mut self.profile {
            profile.invalidate_geometry();
        }
    }
    fn filtered(&self) -> Vec<usize> {
        let needle = self.search.text().to_lowercase();
        self.catalog
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                needle.is_empty()
                    || r.id.to_lowercase().contains(&needle)
                    || r.name.to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }
    fn current(&self) -> Option<&str> {
        self.filtered()
            .get(self.row)
            .map(|i| self.catalog.rows[*i].id.as_str())
    }
    fn move_row(&mut self, delta: isize) {
        self.row = self
            .row
            .saturating_add_signed(delta)
            .min(self.filtered().len().saturating_sub(1));
        self.focus = 1;
    }
}
impl App {
    pub(super) fn enabled_models_enabled(&self, command: &Command) -> bool {
        let Some(dialog) = &self.management.dialog else {
            return false;
        };
        let Some(state) = &dialog.enabled_models else {
            return false;
        };
        if !dialog.visible
            || dialog.blocked
            || self.management.pending.is_some()
            || !self.management_identity(&dialog.target)
        {
            return false;
        }
        match command {
            Command::Profile(command) => state
                .profile
                .as_ref()
                .is_some_and(|profile| profile.accepts(command)),
            Command::Search => true,
            Command::Toggle(id) => {
                state.catalog.ready && state.catalog.rows.iter().any(|row| row.id == *id)
            }
            Command::Retry => {
                state.catalog.error == Some("session-model-load-failed")
                    && self.management.enabled_models_pending.is_none()
            }
        }
    }
    pub(super) fn enabled_models_action(&mut self, command: Command) -> Option<Action> {
        let dialog = self.management.dialog.as_mut()?;
        let state = dialog.enabled_models.as_mut()?;
        match command {
            Command::Profile(command) => {
                if command == profile::Command::Back {
                    state.profile = None;
                    state.focus = 1;
                } else {
                    state.profile.as_mut()?.apply(command);
                }
            }
            Command::Search => state.focus = 0,
            Command::Retry => state.catalog.retry(),
            Command::Toggle(id) => {
                if state.edit_profiles {
                    let model = state.catalog.rows.iter().find(|model| model.id == id)?;
                    state.profile = Some(profile::Draft::new(
                        model,
                        state
                            .catalog
                            .overrides
                            .get(&id)
                            .cloned()
                            .unwrap_or_default(),
                    ));
                    self.hits.clear();
                    dialog.error = None;
                    return None;
                }
                if let Some(index) = state.selected.iter().position(|selected| selected == &id) {
                    state.selected.remove(index);
                } else if state.selected.len() == 512 {
                    dialog.error = Some("onboard-model-limit");
                    return None;
                } else {
                    state.selected.push(id.clone());
                }
                if let Some(index) = state
                    .filtered()
                    .iter()
                    .position(|index| state.catalog.rows[*index].id == id)
                {
                    state.row = index;
                }
                state.focus = 1;
            }
        }
        dialog.error = None;
        state.hovered = None;
        self.hits.clear();
        None
    }
    pub fn enabled_models_request(&mut self) -> Option<Request> {
        if self.management.enabled_models_pending.is_some() || self.management.pending.is_some() {
            return None;
        }
        let dialog = self.management.dialog.as_ref()?;
        if dialog.blocked || !self.management_identity(&dialog.target) {
            return None;
        }
        let dialog = self.management.dialog.as_mut()?;
        let state = dialog.enabled_models.as_mut()?;
        let request = Request {
            generation: state.generation,
            target: dialog.target.clone(),
            query: state.catalog.query()?,
        };
        self.management.enabled_models_pending = Some(request.clone());
        Some(request)
    }
    pub fn enabled_models_completed(&mut self, request: Request, result: Result<Value, String>) {
        if self.management.enabled_models_pending.as_ref() != Some(&request) {
            return;
        }
        self.management.enabled_models_pending = None;
        if !self.management_identity(&request.target) {
            return;
        }
        let Some(dialog) = self
            .management
            .dialog
            .as_mut()
            .filter(|d| d.target == request.target && !d.blocked)
        else {
            return;
        };
        let Some(state) = dialog
            .enabled_models
            .as_mut()
            .filter(|s| s.generation == request.generation)
        else {
            return;
        };
        state.catalog.complete(result);
    }
    pub(super) fn enabled_models_input(&mut self, event: Event) -> (bool, Option<Action>) {
        if self
            .management
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.enabled_models.as_ref())
            .is_some_and(|state| state.profile.is_some())
        {
            return profile::input(self, event);
        }
        let page = self
            .hits
            .iter()
            .filter(|h| {
                matches!(
                    h.action,
                    Action::Manage(Manage::EnabledModels(Command::Toggle(_)))
                )
            })
            .count()
            .max(1);
        let dialog = self
            .management
            .dialog
            .as_mut()
            .expect("enabled models dialog");
        let state = dialog
            .enabled_models
            .as_mut()
            .expect("enabled models state");
        let editable = dialog.visible && !dialog.blocked && self.management.pending.is_none();
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                state.hovered = None;
                match key.code {
                    KeyCode::Esc => Some(Manage::Close),
                    KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return (true, Some(Action::Quit));
                    }
                    _ if !editable => None,
                    KeyCode::Tab => {
                        state.focus = (state.focus + 1) % if state.edit_profiles { 3 } else { 4 };
                        return (true, None);
                    }
                    KeyCode::BackTab => {
                        let controls = if state.edit_profiles { 3 } else { 4 };
                        state.focus = (state.focus + controls - 1) % controls;
                        return (true, None);
                    }
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        Some(Manage::Save)
                    }
                    KeyCode::Enter => match state.focus {
                        0 => {
                            state.focus = 1;
                            return (true, None);
                        }
                        1 => state
                            .current()
                            .map(|id| Manage::EnabledModels(Command::Toggle(id.into()))),
                        2 => Some(Manage::Close),
                        _ => Some(Manage::Save),
                    },
                    KeyCode::Down | KeyCode::Up => {
                        state.move_row(if key.code == KeyCode::Down { 1 } else { -1 });
                        return (true, None);
                    }
                    KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown
                        if state.focus == 1 =>
                    {
                        state.move_row(match key.code {
                            KeyCode::Home => isize::MIN,
                            KeyCode::End => isize::MAX,
                            KeyCode::PageUp => -(page as isize),
                            _ => page as isize,
                        });
                        return (true, None);
                    }
                    KeyCode::Char(' ') if state.focus == 1 => state
                        .current()
                        .map(|id| Manage::EnabledModels(Command::Toggle(id.into()))),
                    KeyCode::F(5) => Some(Manage::EnabledModels(Command::Retry)),
                    _ if state.focus == 0 => {
                        let changed = state.search.key(key);
                        if changed {
                            state.row = 0;
                        }
                        return (changed, None);
                    }
                    _ => None,
                }
            }
            Event::Paste(text) if editable && state.focus == 0 => {
                if text.chars().any(char::is_control) {
                    return (false, None);
                }
                let changed = state.search.insert(&text);
                if changed {
                    state.row = 0;
                }
                return (changed, None);
            }
            Event::Mouse(mouse) if dialog.visible => {
                if editable
                    && matches!(
                        mouse.kind,
                        MouseEventKind::Down(MouseButton::Left)
                            | MouseEventKind::Drag(MouseButton::Left)
                            | MouseEventKind::Up(MouseButton::Left)
                    )
                    && (state.search.contains((mouse.column, mouse.row).into())
                        || state.search.dragging())
                {
                    state.focus = 0;
                    return (state.search.mouse(mouse), None);
                }
                let hit = self
                    .hits
                    .iter()
                    .rev()
                    .find(|h| h.area.contains((mouse.column, mouse.row).into()))
                    .and_then(|h| match &h.action {
                        Action::Manage(c) => Some(c.clone()),
                        _ => None,
                    });
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => hit,
                    MouseEventKind::Moved => {
                        let changed = state.hovered != hit;
                        state.hovered = hit;
                        return (changed, None);
                    }
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                        if editable
                            && matches!(hit, Some(Manage::EnabledModels(Command::Toggle(_)))) =>
                    {
                        state.move_row(if mouse.kind == MouseEventKind::ScrollDown {
                            1
                        } else {
                            -1
                        });
                        return (true, None);
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        (
            command.is_some(),
            command.and_then(|c| self.apply(Action::Manage(c))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Locale, LocalePreference, app::ConnectionState, i18n::I18n, navigation::Route};
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    fn first() -> Value {
        json!({"kind":"page","revision":9,"connectionCount":1,"defaultTarget":{"connectionId":"c","modelId":"kept"},
            "nextCursor":{"part":"catalog_entry","connectionIndex":0,"itemIndex":0},"items":[
            {"kind":"connection","connectionIndex":0,"connectionId":"c","revision":3,"slug":"fixture","name":"Fixture","provider":crate::providers::fixtures::entry("openai-compatible", false).identity,"configuration":{"baseUrl":"http://127.0.0.1/v1"},"enabled":true,"enabledModelIdCount":2,"catalogEntryCount":2},
            {"kind":"enabled_model_id","connectionIndex":0,"itemIndex":0,"modelId":"kept"},
            {"kind":"enabled_model_id","connectionIndex":0,"itemIndex":1,"modelId":"manual"}]})
    }
    fn last() -> Value {
        json!({"kind":"page","revision":9,"connectionCount":1,"defaultTarget":null,"nextCursor":null,"items":[
            {"kind":"catalog_entry","connectionIndex":0,"itemIndex":0,"entry":{"id":"kept"}},
            {"kind":"catalog_entry","connectionIndex":0,"itemIndex":1,"entry":{"id":"new","displayName":"新增模型"}}]})
    }
    #[test]
    fn enabled_model_editor_waits_for_complete_inventory_preserves_manual_ids_and_guards_writes() {
        for locale in Locale::ALL {
            let mut app = App::new(
                "/fixture".into(),
                I18n::new(LocalePreference::Explicit(locale), locale),
            );
            app.connection = ConnectionState::Connected {
                root_id: "root".into(),
                epoch: "epoch".into(),
            };
            app.apply(Action::Visit(Route::Connections));
            app.connections.refresh();
            app.connections.query().unwrap();
            app.connections.complete(Ok(first()));
            app.connections.selected = Some("c".into());
            let open = app
                .management_commands()
                .into_iter()
                .map(|(action, _)| action)
                .find(|action| {
                    matches!(
                        action,
                        Action::Manage(Manage::Open(
                            _,
                            super::super::Kind::Connection(
                                super::super::connection::Change::EnabledModels
                            )
                        ))
                    )
                })
                .unwrap();
            app.apply(open.clone());
            let old = app.enabled_models_request().unwrap();
            app.apply(Action::Manage(Manage::Close));
            app.apply(open);
            assert!(app.enabled_models_request().is_none());
            app.enabled_models_completed(old, Ok(first()));
            let request = app.enabled_models_request().unwrap();
            app.enabled_models_completed(request, Ok(first()));
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
            assert!(!app.management_enabled(&Manage::Save));
            assert!(!app.enabled_models_enabled(&Command::Toggle("manual".into())));
            let request = app.enabled_models_request().unwrap();
            app.enabled_models_completed(request, Ok(last()));
            assert!(
                !app.management_enabled(&Manage::Save),
                "unchanged sets are not writes"
            );
            for (width, height) in [(42, 17), (80, 24), (120, 40)] {
                let mut screen = Terminal::new(TestBackend::new(width, height)).unwrap();
                screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                let hit = app
                    .hits
                    .iter()
                    .find(|h| {
                        h.action
                            == Action::Manage(Manage::EnabledModels(Command::Toggle(
                                "manual".into(),
                            )))
                    })
                    .unwrap()
                    .area;
                assert!(app.modal_area.unwrap().contains((hit.x, hit.y).into()));
                app.input(Event::Mouse(crossterm::event::MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: hit.x,
                    row: hit.y,
                    modifiers: KeyModifiers::NONE,
                }));
                assert!(app.management_enabled(&Manage::Save));
                app.apply(Action::Manage(Manage::EnabledModels(Command::Toggle(
                    "manual".into(),
                ))));
                assert!(
                    !app.management_enabled(&Manage::Save),
                    "reselecting must not silently reorder/write the set"
                );
            }
            app.apply(Action::Manage(Manage::EnabledModels(Command::Search)));
            app.input(Event::Paste("新增".into()));
            terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
            assert_eq!(
                app.hits
                    .iter()
                    .filter(|h| matches!(
                        &h.action,
                        Action::Manage(Manage::EnabledModels(Command::Toggle(_)))
                    ))
                    .count(),
                1
            );
            app.input(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Down,
                KeyModifiers::NONE,
            )));
            app.input(Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char(' '),
                KeyModifiers::NONE,
            )));
            let ticket = app.management_request().unwrap();
            assert_eq!(
                ticket.enabled_model_ids.as_ref().unwrap(),
                &["kept", "manual", "new"]
            );
            app.apply(Action::Manage(Manage::Close));
            assert!(app.management_request().is_none());
            app.management_completed(
                ticket,
                Err(maka_client::RequestFailure::Unknown(
                    maka_client::ClientError::Timeout,
                )),
            );
            assert!(
                app.management.dialog.is_none(),
                "late write outcome must not reopen the dialog"
            );
        }
    }
}
