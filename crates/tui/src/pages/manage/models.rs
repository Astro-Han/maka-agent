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

mod catalog;
mod view;
use super::{Command as Manage, Target};
use crate::app::{Action, App};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use maka_protocol::configuration::ConnectionCatalogQueryInput;
use maka_protocol::session::ThinkingLevel;
use serde_json::Value;
pub(super) use view::draw;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub connection_id: String,
    pub slug: String,
    pub model: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Select(Choice),
    ClearDefault,
    Refresh,
    Previous,
    Next,
    Thinking(bool),
}
impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Select(_) => "session-model-change",
            Self::ClearDefault => "default-model-none",
            Self::Refresh => "command-refresh",
            Self::Previous => "sessions-previous",
            Self::Next => "sessions-next",
            Self::Thinking(_) => "session-thinking",
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    generation: u64,
    target: Target,
    pub query: ConnectionCatalogQueryInput,
}
pub(super) struct Models {
    generation: u64,
    pub catalog: catalog::Catalog,
    focus: usize,
    hovered: Option<Manage>,
    pub for_default: bool,
    pub clear_default: bool,
    pub thinking: Option<ThinkingLevel>,
}
impl Models {
    pub fn new(generation: u64, for_default: bool) -> Self {
        let mut catalog = catalog::Catalog::default();
        catalog.refresh();
        Self {
            generation,
            catalog,
            focus: 0,
            hovered: None,
            for_default,
            clear_default: false,
            thinking: None,
        }
    }
    pub fn selection(&self) -> Option<&catalog::Row> {
        self.catalog.selection()
    }
    pub fn can_submit(&self) -> bool {
        self.selection().is_some()
            || (self.for_default && self.clear_default && self.catalog.revision().is_some())
    }
    fn has_thinking(&self) -> bool {
        !self.for_default
            && self
                .selection()
                .is_some_and(|row| !row.thinking_levels.is_empty())
    }
    pub fn thinking_level(&self) -> Option<ThinkingLevel> {
        self.thinking.filter(|level| {
            !self.for_default
                && self
                    .selection()
                    .is_some_and(|row| row.thinking_levels.contains(level))
        })
    }
    fn cycle_thinking(&mut self, forward: bool) {
        let Some(row) = self.selection().filter(|_| self.has_thinking()) else {
            return;
        };
        let current = self
            .thinking_level()
            .and_then(|level| row.thinking_levels.iter().position(|v| *v == level))
            .map_or(0, |i| i + 1);
        let count = row.thinking_levels.len() + 1;
        let next = (current + if forward { 1 } else { count - 1 }) % count;
        self.thinking = next.checked_sub(1).map(|i| row.thinking_levels[i]);
    }
    fn tab(&mut self, forward: bool) {
        // Stable control identities: removing thinking must never turn Cancel into Save.
        let order: Vec<_> = [0, 1, 2, 3, 6, 4, 5]
            .into_iter()
            .filter(|id| *id != 6 || self.has_thinking())
            .collect();
        let index = order.iter().position(|id| *id == self.focus).unwrap_or(0);
        self.focus = order[(index + if forward { 1 } else { order.len() - 1 }) % order.len()];
    }
    fn refresh(&mut self) {
        if self.for_default {
            self.clear_default = false;
            self.catalog.selected = None;
        }
        self.catalog.refresh();
    }
    fn move_selection(&mut self, down: bool) {
        if self.for_default
            && !down
            && self
                .catalog
                .rows
                .first()
                .is_some_and(|row| Some(&row.choice) == self.catalog.selected.as_ref())
        {
            self.clear_default = true;
            self.catalog.selected = None;
        } else if self.for_default && self.clear_default && !down {
            // The no-default choice is the first row.
        } else {
            self.clear_default = false;
            self.catalog.move_selection(down);
        }
    }
    pub fn invalidate_geometry(&mut self) {
        self.hovered = None;
    }
}

impl App {
    pub fn default_model_action(&self) -> Option<Action> {
        let crate::app::ConnectionState::Connected { root_id, epoch } = &self.connection else {
            return None;
        };
        Some(Action::Manage(Manage::Open(
            Target {
                root: root_id.clone(),
                epoch: epoch.clone(),
                name: self.i18n.text("default-model-title"),
                entity: super::Entity::Defaults,
            },
            super::Kind::Model,
        )))
    }
    pub fn models_catalog_changed(&mut self) {
        if let Some(models) = self
            .management
            .dialog
            .as_mut()
            .and_then(|d| d.models.as_mut())
        {
            models.refresh();
        }
    }
    pub fn model_action(&self) -> Option<Action> {
        self.management_commands().into_iter().find_map(|(a, _)| {
            matches!(a, Action::Manage(Manage::Open(_, super::Kind::Model))).then_some(a)
        })
    }
    pub(super) fn models_enabled(&self, command: &Command) -> bool {
        let Some(dialog) = &self.management.dialog else {
            return false;
        };
        let Some(models) = &dialog.models else {
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
            Command::ClearDefault => models.for_default && models.catalog.revision().is_some(),
            Command::Select(choice) => models.catalog.rows.iter().any(|r| r.choice == *choice),
            Command::Refresh => !models.catalog.loading,
            Command::Previous => models.catalog.can_previous(),
            Command::Next => models.catalog.can_next(),
            Command::Thinking(_) => models.has_thinking(),
        }
    }
    pub(super) fn models_action(&mut self, command: Command) -> Option<Action> {
        let dialog = self.management.dialog.as_mut()?;
        let models = dialog.models.as_mut()?;
        match command {
            Command::Select(choice) => {
                models.clear_default = false;
                models.catalog.selected = Some(choice);
                models.focus = 0;
            }
            Command::ClearDefault => {
                models.clear_default = true;
                models.catalog.selected = None;
                models.focus = 0;
            }
            Command::Refresh => models.refresh(),
            Command::Previous => models.catalog.change_page(false),
            Command::Next => models.catalog.change_page(true),
            Command::Thinking(forward) => {
                models.cycle_thinking(forward);
                models.focus = 6;
            }
        }
        models.hovered = None;
        dialog.error = None;
        self.hits.clear();
        None
    }
    pub fn models_request(&mut self) -> Option<Request> {
        if self.management.models_pending.is_some() || self.management.pending.is_some() {
            return None;
        }
        let dialog = self.management.dialog.as_ref()?;
        if dialog.blocked || !self.management_identity(&dialog.target) {
            return None;
        }
        let dialog = self.management.dialog.as_mut()?;
        let models = dialog.models.as_mut()?;
        let request = Request {
            generation: models.generation,
            target: dialog.target.clone(),
            query: models.catalog.query()?,
        };
        self.management.models_pending = Some(request.clone());
        Some(request)
    }
    pub fn models_completed(&mut self, request: Request, result: Result<Value, String>) {
        if self.management.models_pending.as_ref() != Some(&request) {
            return;
        }
        self.management.models_pending = None;
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
        let Some(models) = dialog
            .models
            .as_mut()
            .filter(|m| m.generation == request.generation)
        else {
            return;
        };
        if models.for_default
            && result
                .as_ref()
                .is_ok_and(|page| page["kind"] == "revision_changed")
        {
            models.clear_default = false;
            models.catalog.selected = None;
        }
        models.catalog.complete(result);
        if models.focus == 6 && !models.has_thinking() {
            models.focus = 0;
        }
    }
    pub(super) fn models_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let dialog = self.management.dialog.as_mut().expect("models dialog");
        let models = dialog.models.as_mut().expect("models chooser");
        let movable = !dialog.blocked && self.management.pending.is_none();
        if matches!(&event,Event::Key(k) if k.kind!=KeyEventKind::Release) {
            models.hovered = None;
        }
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc => Some(Manage::Close),
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                _ if !dialog.visible => None,
                KeyCode::Tab => {
                    models.tab(true);
                    return (true, None);
                }
                KeyCode::BackTab => {
                    models.tab(false);
                    return (true, None);
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
                    if movable && models.focus == 6 && models.has_thinking() =>
                {
                    Some(Manage::Models(Command::Thinking(matches!(
                        key.code,
                        KeyCode::Right | KeyCode::Down
                    ))))
                }
                KeyCode::Up | KeyCode::Down if movable => {
                    models.move_selection(key.code == KeyCode::Down);
                    models.focus = 0;
                    dialog.error = None;
                    return (true, None);
                }
                KeyCode::Home | KeyCode::End if movable => {
                    models.clear_default = models.for_default
                        && (key.code == KeyCode::Home || models.catalog.rows.is_empty());
                    models.catalog.selected = if models.clear_default {
                        None
                    } else if key.code == KeyCode::Home {
                        models.catalog.rows.first()
                    } else {
                        models.catalog.rows.last()
                    }
                    .map(|r| r.choice.clone());
                    models.focus = 0;
                    dialog.error = None;
                    return (true, None);
                }
                KeyCode::F(5) => Some(Manage::Models(Command::Refresh)),
                KeyCode::PageUp => Some(Manage::Models(Command::Previous)),
                KeyCode::PageDown => Some(Manage::Models(Command::Next)),
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(Manage::Save)
                }
                KeyCode::Enter => Some(focused(models)),
                _ => None,
            },
            Event::Mouse(mouse) if dialog.visible => {
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
                        let changed = models.hovered != hit;
                        models.hovered = hit;
                        return (changed, None);
                    }
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                        if movable && matches!(hit, Some(Manage::Models(Command::Select(_)))) =>
                    {
                        models.move_selection(mouse.kind == MouseEventKind::ScrollDown);
                        models.focus = 0;
                        dialog.error = None;
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
fn focused(models: &Models) -> Manage {
    match models.focus {
        1 => Manage::Models(Command::Refresh),
        2 => Manage::Models(Command::Previous),
        3 => Manage::Models(Command::Next),
        4 => Manage::Close,
        6 if models.has_thinking() => Manage::Models(Command::Thinking(true)),
        _ => Manage::Save,
    }
}

pub(crate) fn thinking_key(level: Option<ThinkingLevel>) -> &'static str {
    match level {
        None => "thinking-default",
        Some(ThinkingLevel::Off) => "thinking-off",
        Some(ThinkingLevel::Minimal) => "thinking-minimal",
        Some(ThinkingLevel::Low) => "thinking-low",
        Some(ThinkingLevel::Medium) => "thinking-medium",
        Some(ThinkingLevel::High) => "thinking-high",
        Some(ThinkingLevel::Xhigh) => "thinking-xhigh",
        Some(ThinkingLevel::Max) => "thinking-max",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::ConnectionState,
        i18n::{I18n, Locale, LocalePreference},
        pages::manage::{Entity, Kind},
    };
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    fn page(model: &str) -> Value {
        json!({"kind":"page","revision":7,"nextCursor":null,"items":[
            {"kind":"connection","connectionIndex":0,"connectionId":"connection","slug":"fixture","name":"Fixture","enabled":true},
            {"kind":"enabled_model_id","connectionIndex":0,"modelId":model},
            {"kind":"catalog_entry","connectionIndex":0,"entry":{"id":model,"canUseAsChatDefault":true}}
        ]})
    }
    #[test]
    fn thinking_uses_current_catalog_levels_and_rechecks_before_submission() {
        for locale in Locale::ALL {
            let mut app = App::new(
                "/unused".into(),
                I18n::new(LocalePreference::Explicit(locale), locale),
            );
            app.connection = ConnectionState::Connected {
                root_id: "root".into(),
                epoch: "epoch".into(),
            };
            let target = Target {
                root: "root".into(),
                epoch: "epoch".into(),
                name: "Session".into(),
                entity: Entity::Session {
                    id: "session".into(),
                    revision: 7,
                    workspace: "/work".into(),
                    project_bound: false,
                },
            };
            app.apply(Action::Manage(Manage::Open(target, Kind::Model)));
            let mut data = page("model");
            data["items"][2]["entry"]["thinkingLevels"] = json!(["low", "high"]);
            let request = app.models_request().unwrap();
            app.models_completed(request, Ok(data.clone()));
            let models = app
                .management
                .dialog
                .as_mut()
                .unwrap()
                .models
                .as_mut()
                .unwrap();
            models.thinking = Some(ThinkingLevel::Low);
            let choice = models.catalog.rows[0].choice.clone();
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
            app.apply(Action::Manage(Manage::Models(Command::Select(choice))));
            for (width, height) in [(42, 17), (80, 24), (120, 40)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                let hit = app
                    .hits
                    .iter()
                    .find(|h| h.action == Action::Manage(Manage::Models(Command::Thinking(true))))
                    .unwrap()
                    .area;
                assert!(app.modal_area.unwrap().contains((hit.x, hit.y).into()));
                assert!(!app.hits.iter().any(|h| matches!(
                    &h.action,
                    Action::Manage(Manage::Models(Command::Select(_)))
                ) && h.area.intersects(hit)));
                app.input(Event::Mouse(crossterm::event::MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: hit.x,
                    row: hit.y,
                    modifiers: KeyModifiers::NONE,
                }));
                assert_eq!(
                    app.management
                        .dialog
                        .as_ref()
                        .unwrap()
                        .models
                        .as_ref()
                        .unwrap()
                        .thinking_level(),
                    Some(ThinkingLevel::High)
                );
                terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                let previous = app
                    .hits
                    .iter()
                    .find(|h| h.action == Action::Manage(Manage::Models(Command::Thinking(false))))
                    .unwrap()
                    .area;
                app.input(Event::Mouse(crossterm::event::MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: previous.x + 1,
                    row: previous.y,
                    modifiers: KeyModifiers::NONE,
                }));
                assert_eq!(
                    app.management
                        .dialog
                        .as_ref()
                        .unwrap()
                        .models
                        .as_ref()
                        .unwrap()
                        .thinking_level(),
                    Some(ThinkingLevel::Low)
                );
                app.input(Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Left,
                    KeyModifiers::NONE,
                )));
                assert_eq!(
                    app.management
                        .dialog
                        .as_ref()
                        .unwrap()
                        .models
                        .as_ref()
                        .unwrap()
                        .thinking_level(),
                    None
                );
                app.input(Event::Key(crossterm::event::KeyEvent::new(
                    KeyCode::Right,
                    KeyModifiers::NONE,
                )));
                assert_eq!(
                    app.management
                        .dialog
                        .as_ref()
                        .unwrap()
                        .models
                        .as_ref()
                        .unwrap()
                        .thinking_level(),
                    Some(ThinkingLevel::Low)
                );
            }
            // A catalog update withdraws the old controls and must not submit an obsolete level.
            app.models_catalog_changed();
            assert!(!app.models_enabled(&Command::Thinking(true)));
            assert!(!app.management_enabled(&Manage::Save));
            data["items"][2]["entry"]["thinkingLevels"] = json!(["high"]);
            let request = app.models_request().unwrap();
            app.models_completed(request, Ok(data));
            let models = app
                .management
                .dialog
                .as_ref()
                .unwrap()
                .models
                .as_ref()
                .unwrap();
            assert_eq!(models.thinking, Some(ThinkingLevel::Low));
            assert_eq!(models.thinking_level(), None);
            let models = app
                .management
                .dialog
                .as_mut()
                .unwrap()
                .models
                .as_mut()
                .unwrap();
            models.focus = 4;
            assert_eq!(focused(models), Manage::Close);
            models.catalog.rows[0].thinking_levels.clear();
            assert!(!models.has_thinking());
            assert_eq!(focused(models), Manage::Close);
            models.tab(true);
            assert_eq!(focused(models), Manage::Save);
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
            let ticket = app.management_request().unwrap();
            assert_eq!(ticket.thinking_level, None);
            assert_eq!(ticket.model.unwrap().model, "model");
        }
        let mut defaults = Models::new(1, true);
        defaults.catalog.query().unwrap();
        let mut data = page("model");
        data["items"][2]["entry"]["thinkingLevels"] = json!(["high"]);
        defaults.catalog.complete(Ok(data));
        defaults.catalog.move_selection(true);
        defaults.thinking = Some(ThinkingLevel::High);
        assert!(!defaults.has_thinking());
        assert_eq!(defaults.thinking_level(), None);
    }
    #[test]
    fn default_model_requires_explicit_fresh_intent_and_preserves_unknown_write_guard() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(app.default_model_action().unwrap());
        let request = app.models_request().unwrap();
        app.models_completed(request, Ok(page("model")));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        assert!(
            !app.management_enabled(&Manage::Save),
            "neither no-default nor first row is implicit"
        );
        app.apply(Action::Manage(Manage::Models(Command::ClearDefault)));
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            for (width, height) in [(80, 24), (42, 17), (20, 8)] {
                let mut screen = Terminal::new(TestBackend::new(width, height)).unwrap();
                screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                assert_eq!(app.management_enabled(&Manage::Save), width >= 42);
            }
        }
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        app.models_catalog_changed();
        let request = app.models_request().unwrap();
        app.models_completed(request, Ok(page("model")));
        assert!(
            !app.management_enabled(&Manage::Save),
            "configuration notification withdraws clear intent"
        );
        app.input(Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::NONE,
        )));
        assert!(app.management_enabled(&Manage::Save));
        app.models_catalog_changed();
        let request = app.models_request().unwrap();
        app.models_completed(request, Ok(page("model")));
        assert!(
            !app.management_enabled(&Manage::Save),
            "same model identity cannot silently retain old default intent"
        );
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let hit = app
            .hits
            .iter()
            .find(|h| h.action == Action::Manage(Manage::Models(Command::ClearDefault)))
            .unwrap()
            .area;
        app.input(Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        }));
        let ticket = app.management_request().unwrap();
        assert!(ticket.target.is_default_model());
        assert_eq!(ticket.catalog_revision, Some(7));
        assert!(ticket.model.is_none());
        app.management_completed(
            ticket,
            Err(maka_client::RequestFailure::Unknown(
                maka_client::ClientError::Timeout,
            )),
        );
        app.models_catalog_changed();
        assert!(app.models_request().is_none());
        assert!(!app.management_enabled(&Manage::Save));
        app.apply(Action::Manage(Manage::Close));
        app.apply(app.default_model_action().unwrap());
        assert!(
            app.models_request().is_some(),
            "reopening requires an authoritative read"
        );
    }
    #[test]
    fn model_dialog_binds_generation_target_and_fresh_mouse_intent() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        let target = Target {
            root: "root".into(),
            epoch: "epoch".into(),
            name: "Session".into(),
            entity: Entity::Session {
                id: "session".into(),
                revision: 7,
                workspace: "/work".into(),
                project_bound: false,
            },
        };
        let open = Action::Manage(Manage::Open(target.clone(), Kind::Model));
        app.apply(open.clone());
        let old = app.models_request().unwrap();
        app.apply(Action::Manage(Manage::Close));
        app.apply(open);
        assert!(app.models_request().is_none());
        app.models_completed(old, Ok(page("old")));
        let request = app.models_request().unwrap();
        app.models_completed(request, Ok(page("model")));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        assert!(
            app.management_request().is_none(),
            "no implicit first choice"
        );
        app.models_catalog_changed();
        let request = app.models_request().unwrap();
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let choice = Choice {
            connection_id: "connection".into(),
            slug: "fixture".into(),
            model: "model".into(),
        };
        let hit = app
            .hits
            .iter()
            .find(|h| h.action == Action::Manage(Manage::Models(Command::Select(choice.clone()))))
            .unwrap()
            .area;
        app.input(Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(!app.management_enabled(&Manage::Save));
        app.models_completed(request, Ok(page("model")));
        assert!(
            app.management_enabled(&Manage::Save),
            "local click survives refresh of the same identity"
        );
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            for (width, height) in [(80, 24), (42, 17), (20, 8)] {
                let mut screen = Terminal::new(TestBackend::new(width, height)).unwrap();
                screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                assert_eq!(app.management_enabled(&Manage::Save), width >= 42);
            }
        }
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        app.models_catalog_changed();
        let request = app.models_request().unwrap();
        app.models_completed(request, Ok(page("replacement")));
        assert!(
            !app.management_enabled(&Manage::Save),
            "removed choice cannot silently select its neighbour"
        );
        app.input(Event::Key(crossterm::event::KeyEvent::new(
            KeyCode::Down,
            KeyModifiers::NONE,
        )));
        let ticket = app.management_request().unwrap();
        assert_eq!(ticket.target, target);
        assert_eq!(ticket.model.as_ref().unwrap().model, "replacement");
        assert!(app.management_request().is_none());
        app.management_completed(
            ticket,
            Err(maka_client::RequestFailure::Unknown(
                maka_client::ClientError::Timeout,
            )),
        );
        app.models_catalog_changed();
        assert!(app.models_request().is_none());
        assert!(
            !app.management_enabled(&Manage::Save),
            "unknown writes never become replayable after refresh"
        );
    }
}
