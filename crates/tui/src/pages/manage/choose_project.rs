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

mod view;
pub(super) use view::sheet;

use super::{Command as Manage, Target};
use crate::{
    app::{Action, App},
    pages::projects::{Item, Projects},
};
use maka_protocol::project::{Query, QueryResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Select(String),
    Refresh,
    Previous,
    Next,
}
impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Select(_) => "project-select",
            Self::Refresh => "command-refresh",
            Self::Previous => "sessions-previous",
            Self::Next => "sessions-next",
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    generation: u64,
    target: Target,
    pub query: Query,
}
pub(super) struct Chooser {
    generation: u64,
    pub catalog: Projects,
}
impl Chooser {
    pub fn new(generation: u64) -> Self {
        let mut catalog = Projects::default();
        catalog.refresh();
        Self {
            generation,
            catalog,
        }
    }
    pub fn selection(&self) -> Option<&Item> {
        self.catalog.ready().then_some(())?;
        self.catalog
            .items
            .iter()
            .find(|item| Some(&item.id) == self.catalog.selected.as_ref() && item.usable())
    }
}

impl App {
    pub fn project_catalog_changed(&mut self) {
        self.projects.refresh();
        if let Some(locations) = self
            .management
            .dialog
            .as_mut()
            .and_then(|d| d.locations.as_mut())
        {
            locations.refresh();
        }
        if let Some(chooser) = self
            .management
            .dialog
            .as_mut()
            .and_then(|d| d.chooser.as_mut())
        {
            chooser.catalog.refresh();
        }
    }
    pub(super) fn choose_project_enabled(&self, command: &Command) -> bool {
        let Some(dialog) = &self.management.dialog else {
            return false;
        };
        let Some(chooser) = &dialog.chooser else {
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
            Command::Select(id) => chooser.catalog.items.iter().any(|i| i.id == *id),
            Command::Refresh => !chooser.catalog.loading,
            Command::Previous => {
                chooser.catalog.can_previous() && (chooser.catalog.ready() || chooser.catalog.error)
            }
            Command::Next => chooser.catalog.ready() && chooser.catalog.can_next(),
        }
    }
    pub(super) fn choose_project_action(&mut self, command: Command) -> Option<Action> {
        let dialog = self.management.dialog.as_mut()?;
        let chooser = dialog.chooser.as_mut()?;
        match command {
            Command::Select(id) => chooser.catalog.selected = Some(id),
            Command::Refresh => chooser.catalog.restart(),
            Command::Previous => chooser.catalog.change_page(false),
            Command::Next => chooser.catalog.change_page(true),
        }
        dialog.error = None;
        None
    }
    pub fn choose_project_request(&mut self) -> Option<Request> {
        if self.management.chooser_pending.is_some() || self.management.pending.is_some() {
            return None;
        }
        let dialog = self.management.dialog.as_ref()?;
        if dialog.blocked || !self.management_identity(&dialog.target) {
            return None;
        }
        let dialog = self.management.dialog.as_mut()?;
        let chooser = dialog.chooser.as_mut()?;
        let request = Request {
            generation: chooser.generation,
            target: dialog.target.clone(),
            query: chooser.catalog.query()?,
        };
        self.management.chooser_pending = Some(request.clone());
        Some(request)
    }
    pub fn choose_project_completed(
        &mut self,
        request: Request,
        result: Result<QueryResult, String>,
    ) {
        if self.management.chooser_pending.as_ref() != Some(&request) {
            return;
        }
        self.management.chooser_pending = None;
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
        let Some(chooser) = dialog
            .chooser
            .as_mut()
            .filter(|c| c.generation == request.generation)
        else {
            return;
        };
        let first = !chooser.catalog.loaded;
        chooser.catalog.complete(result);
        if first {
            chooser.catalog.selected = None;
        } // A newly opened chooser never implicitly commits the first project.
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Entity, Kind};
    use super::*;
    use crate::{Locale, LocalePreference, app::ConnectionState, i18n::I18n};
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEventKind};
    use maka_protocol::project::{PageItem, View};
    use ratatui::{Terminal, backend::TestBackend};

    fn page(ids: &[&str]) -> QueryResult {
        QueryResult::Page {
            view: View::Summary,
            revision: "r1".into(),
            project_count: ids.len() as u64,
            items: ids
                .iter()
                .enumerate()
                .map(|(index, id)| PageItem::Project {
                    project_index: index as u64,
                    id: (*id).into(),
                    name: (*id).into(),
                    alias_count: 0,
                    location_count: 1,
                    preferred_location_index: Some(0),
                    archived_at: None,
                    available: true,
                })
                .collect(),
            next_cursor: None,
        }
    }

    #[test]
    fn chooser_pins_session_and_requires_fresh_explicit_project_selection() {
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
        let open = Action::Manage(Manage::Open(target.clone(), Kind::Project));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        app.apply(open.clone());
        let old = app.choose_project_request().unwrap();
        app.apply(Action::Manage(Manage::Close));
        app.apply(open);
        assert!(
            app.choose_project_request().is_none(),
            "closed reads still occupy the single slot"
        );
        app.choose_project_completed(old, Ok(page(&["old"])));
        let request = app.choose_project_request().unwrap();
        app.choose_project_completed(request, Ok(page(&["a", "b"])));
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        assert!(
            app.management_request().is_none(),
            "opening never chooses the first project"
        );
        app.project_catalog_changed();
        let request = app.choose_project_request().unwrap();
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        // The row whose whole text inside the sheet is the project name.
        let buffer = terminal.backend().buffer();
        let (x, y) = (0..buffer.area.height)
            .find_map(|y| {
                let line: String = (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                let inside = line.split('│').nth(1)?;
                (inside.trim() == "b").then(|| {
                    let x = line.find('│').unwrap() + '│'.len_utf8() + inside.find('b').unwrap();
                    (line[..x].chars().count() as u16, y)
                })
            })
            .expect("row b is on screen");
        app.input(Event::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(
            app.management
                .dialog
                .as_ref()
                .unwrap()
                .chooser
                .as_ref()
                .unwrap()
                .catalog
                .selected
                .as_deref(),
            Some("b"),
            "refresh must not drop a visible row's local selection"
        );
        assert!(!app.management_enabled(&Manage::Save));
        app.choose_project_completed(request, Ok(page(&["a", "b"])));
        assert!(app.management_enabled(&Manage::Save));
        app.project_catalog_changed();
        assert!(!app.management_enabled(&Manage::Save));
        let request = app.choose_project_request().unwrap();
        app.project_catalog_changed();
        app.choose_project_completed(request, Ok(page(&["b", "a"])));
        assert!(
            !app.management_enabled(&Manage::Save),
            "notification during read requires a replacement"
        );
        let request = app.choose_project_request().unwrap();
        app.choose_project_completed(request, Ok(page(&["b", "a"])));
        assert!(app.management_enabled(&Manage::Save));
        let chooser = app
            .management
            .dialog
            .as_mut()
            .unwrap()
            .chooser
            .as_mut()
            .unwrap();
        assert_eq!(chooser.catalog.selected.as_deref(), Some("b"));
        chooser.catalog.items[0].archived = true;
        assert!(!app.management_enabled(&Manage::Save));
        let chooser = app
            .management
            .dialog
            .as_mut()
            .unwrap()
            .chooser
            .as_mut()
            .unwrap();
        chooser.catalog.items[0].archived = false;
        chooser.catalog.items[0].available = false;
        assert!(!app.management_enabled(&Manage::Save));
        app.project_catalog_changed();
        let request = app.choose_project_request().unwrap();
        app.choose_project_completed(request, Ok(page(&["a"])));
        assert!(
            !app.management_enabled(&Manage::Save),
            "a vanished selection cannot choose its neighbour"
        );
        app.apply(Action::Manage(Manage::ChooseProject(Command::Select(
            "a".into(),
        ))));
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            for (width, height) in [(80, 24), (42, 17), (20, 8)] {
                let mut screen = Terminal::new(TestBackend::new(width, height)).unwrap();
                screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                assert_eq!(app.management_enabled(&Manage::Save), width >= 42);
            }
        }
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let ticket = app.management_request().unwrap();
        assert_eq!(ticket.target, target);
        assert_eq!(ticket.project_id.as_deref(), Some("a"));
        assert!(app.management_request().is_none());
        app.management_completed(
            ticket,
            Err(maka_client::RequestFailure::Unknown(
                maka_client::ClientError::Timeout,
            )),
        );
        app.project_catalog_changed();
        assert!(app.choose_project_request().is_none());
        assert!(!app.management_enabled(&Manage::Save));
        assert!(!app.choose_project_enabled(&Command::Select("a".into())));
    }
}
