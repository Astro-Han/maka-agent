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
pub(super) use view::draw;

use super::{Command as Manage, Entity, Target};
use crate::app::{Action, App};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use maka_protocol::project::{Location, PageItem, Query, QueryResult, View};
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Refresh,
    Previous,
    Next,
    Scroll(bool),
}
impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Refresh => "command-refresh",
            Self::Previous => "sessions-previous",
            Self::Next => "sessions-next",
            Self::Scroll(_) => "project-locations",
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    generation: u64,
    target: Target,
    pub query: Query,
}

pub(super) struct Locations {
    generation: u64,
    index: Option<u64>,
    name: Option<String>,
    preferred: Option<u64>,
    count: u64,
    rows: Vec<(u64, Location)>,
    revision: Option<String>,
    cursor: Option<String>,
    next: Option<String>,
    previous: VecDeque<Option<String>>,
    requested: bool,
    restart: bool,
    loading: bool,
    error: Option<&'static str>,
    offset: usize,
    max_offset: usize,
    focus: usize, // Content, refresh, previous, next, close.
    hovered: Option<Manage>,
}
impl Locations {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            index: None,
            name: None,
            preferred: None,
            count: 0,
            rows: vec![],
            revision: None,
            cursor: None,
            next: None,
            previous: VecDeque::new(),
            requested: true,
            restart: false,
            loading: false,
            error: None,
            offset: 0,
            max_offset: 0,
            focus: 0,
            hovered: None,
        }
    }
    pub fn refresh(&mut self) {
        self.restart = true;
        self.requested = true;
    }
    pub fn invalidate_geometry(&mut self) {
        self.hovered = None;
        self.max_offset = 0;
    }
    fn ready(&self) -> bool {
        !self.loading && !self.requested && self.error.is_none()
    }
    fn query(&mut self) -> Option<Query> {
        if self.loading || !self.requested {
            return None;
        }
        if std::mem::take(&mut self.restart) {
            self.index = None;
            self.name = None;
            self.preferred = None;
            self.count = 0;
            self.rows.clear();
            self.revision = None;
            self.cursor = None;
            self.next = None;
            self.previous.clear();
            self.offset = 0;
        }
        self.loading = true;
        self.requested = false;
        self.error = None;
        Some(match (&self.revision, &self.cursor) {
            (Some(revision), Some(cursor)) => Query::ListContinue {
                view: View::Locations,
                revision: revision.clone(),
                cursor: cursor.clone(),
            },
            _ => Query::ListStart {
                view: View::Locations,
            },
        })
    }
    fn complete(&mut self, id: &str, result: Result<QueryResult, String>) {
        self.loading = false;
        if self.restart {
            return;
        } // A notification supersedes the whole indexed snapshot.
        match result {
            Ok(QueryResult::RevisionChanged { .. }) => self.refresh(),
            Err(_) => self.error = Some("projects-failed"),
            Ok(QueryResult::Page {
                revision,
                items,
                next_cursor,
                ..
            }) => {
                self.revision = Some(revision);
                self.rows.clear();
                for item in items {
                    match item {
                        PageItem::Project {
                            project_index,
                            id: actual,
                            name,
                            preferred_location_index,
                            location_count,
                            ..
                        } if actual == id => {
                            self.index = Some(project_index);
                            self.name = Some(name);
                            self.preferred = preferred_location_index;
                            self.count = location_count;
                        }
                        PageItem::Location {
                            project_index,
                            item_index,
                            location,
                        } if Some(project_index) == self.index => {
                            self.rows.push((item_index, location))
                        }
                        _ => {}
                    }
                }
                if self.index.is_none() || self.rows.is_empty() && self.count > 0 {
                    if let Some(next) = next_cursor {
                        self.cursor = Some(next);
                        self.requested = true;
                    } else {
                        self.error = Some("project-locations-missing");
                    }
                    return;
                }
                self.next = next_cursor.filter(|_| {
                    self.rows
                        .last()
                        .is_some_and(|(index, _)| index + 1 < self.count)
                });
                self.offset = 0;
            }
            _ => unreachable!("client enforces locations reply"),
        }
    }
}

impl App {
    pub(super) fn locations_enabled(&self, command: &Command) -> bool {
        let Some(dialog) = &self.management.dialog else {
            return false;
        };
        let Some(locations) = &dialog.locations else {
            return false;
        };
        if !dialog.visible || dialog.blocked || !self.management_identity(&dialog.target) {
            return false;
        }
        match command {
            Command::Refresh => !locations.loading,
            Command::Previous => {
                !locations.loading && !locations.requested && !locations.previous.is_empty()
            }
            Command::Next => locations.ready() && locations.next.is_some(),
            Command::Scroll(down) => {
                locations.ready()
                    && if *down {
                        locations.offset < locations.max_offset
                    } else {
                        locations.offset > 0
                    }
            }
        }
    }
    pub(super) fn locations_action(&mut self, command: Command) -> Option<Action> {
        let locations = self.management.dialog.as_mut()?.locations.as_mut()?;
        match command {
            Command::Refresh => locations.refresh(),
            Command::Next => {
                if locations.previous.len() == 128 {
                    locations.previous.pop_front();
                }
                locations.previous.push_back(locations.cursor.clone());
                locations.cursor = locations.next.clone();
                locations.requested = true;
            }
            Command::Previous => {
                locations.cursor = locations.previous.pop_back()?;
                locations.requested = true;
            }
            Command::Scroll(down) => {
                locations.offset = if down {
                    (locations.offset + 3).min(locations.max_offset)
                } else {
                    locations.offset.saturating_sub(3)
                };
            }
        }
        locations.hovered = None;
        self.hits.clear();
        None
    }
    pub fn locations_request(&mut self) -> Option<Request> {
        if self.management.locations_pending.is_some() {
            return None;
        }
        let dialog = self.management.dialog.as_ref()?;
        if dialog.blocked || !self.management_identity(&dialog.target) {
            return None;
        }
        let dialog = self.management.dialog.as_mut()?;
        let locations = dialog.locations.as_mut()?;
        let request = Request {
            generation: locations.generation,
            target: dialog.target.clone(),
            query: locations.query()?,
        };
        self.management.locations_pending = Some(request.clone());
        Some(request)
    }
    pub fn locations_completed(&mut self, request: Request, result: Result<QueryResult, String>) {
        if self.management.locations_pending.as_ref() != Some(&request) {
            return;
        }
        self.management.locations_pending = None;
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
        let Some(locations) = dialog
            .locations
            .as_mut()
            .filter(|l| l.generation == request.generation)
        else {
            return;
        };
        let Entity::Project { id } = &request.target.entity else {
            return;
        };
        locations.complete(id, result);
    }
    pub(super) fn locations_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let dialog = self.management.dialog.as_mut().expect("locations dialog");
        let locations = dialog.locations.as_mut().expect("locations reader");
        if matches!(&event,Event::Key(k) if k.kind!=KeyEventKind::Release) {
            locations.hovered = None;
        }
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc => Some(Manage::Close),
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                _ if !dialog.visible => None,
                KeyCode::Tab => {
                    locations.focus = (locations.focus + 1) % 5;
                    return (true, None);
                }
                KeyCode::BackTab => {
                    locations.focus = (locations.focus + 4) % 5;
                    return (true, None);
                }
                KeyCode::Up => Some(Manage::Locations(Command::Scroll(false))),
                KeyCode::Down => Some(Manage::Locations(Command::Scroll(true))),
                KeyCode::Home | KeyCode::End if locations.ready() => {
                    locations.offset = if key.code == KeyCode::Home {
                        0
                    } else {
                        locations.max_offset
                    };
                    return (true, None);
                }
                KeyCode::PageUp => Some(Manage::Locations(Command::Previous)),
                KeyCode::PageDown => Some(Manage::Locations(Command::Next)),
                KeyCode::F(5) => Some(Manage::Locations(Command::Refresh)),
                KeyCode::Enter => Some(focused(locations)),
                _ => None,
            },
            Event::Mouse(mouse) if dialog.visible => {
                let hit = self
                    .hits
                    .iter()
                    .rev()
                    .find(|h| h.area.contains((mouse.column, mouse.row).into()))
                    .and_then(|h| match &h.action {
                        Action::Manage(command) => Some(command.clone()),
                        _ => None,
                    });
                match mouse.kind {
                    MouseEventKind::Moved => {
                        let changed = locations.hovered != hit;
                        locations.hovered = hit;
                        return (changed, None);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        hit.filter(|c| !matches!(c, Manage::Locations(Command::Scroll(_))))
                    }
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                        if matches!(hit, Some(Manage::Locations(Command::Scroll(_)))) =>
                    {
                        Some(Manage::Locations(Command::Scroll(
                            mouse.kind == MouseEventKind::ScrollDown,
                        )))
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
fn focused(locations: &Locations) -> Manage {
    match locations.focus {
        1 => Manage::Locations(Command::Refresh),
        2 => Manage::Locations(Command::Previous),
        3 => Manage::Locations(Command::Next),
        _ => Manage::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Locale, LocalePreference, app::ConnectionState, i18n::I18n};
    use ratatui::{Terminal, backend::TestBackend};

    fn project(index: u64, id: &str, count: u64) -> PageItem {
        PageItem::Project {
            project_index: index,
            id: id.into(),
            name: id.into(),
            alias_count: 0,
            location_count: count,
            preferred_location_index: (count > 0).then_some(0),
            archived_at: None,
            available: count > 0,
        }
    }
    fn location(project: u64, index: u64, path: &str) -> PageItem {
        PageItem::Location {
            project_index: project,
            item_index: index,
            location: Location {
                path: path.into(),
                is_worktree: false,
            },
        }
    }
    fn page(items: Vec<PageItem>, next: Option<&str>) -> Result<QueryResult, String> {
        Ok(QueryResult::Page {
            view: View::Locations,
            revision: "r1".into(),
            project_count: 3,
            items,
            next_cursor: next.map(str::to_owned),
        })
    }

    #[test]
    fn locations_scan_split_headers_and_aliases_without_caching_other_projects_or_stale_indices() {
        let mut state = Locations::new(1);
        assert!(matches!(
            state.query(),
            Some(Query::ListStart {
                view: View::Locations
            })
        ));
        state.complete(
            "target",
            page(
                vec![project(0, "other", 1), location(0, 0, "/other")],
                Some("2"),
            ),
        );
        assert!(state.rows.is_empty());
        assert!(matches!(state.query(),Some(Query::ListContinue {cursor,..}) if cursor=="2"));
        state.complete("target", page(vec![project(1, "target", 2)], Some("3")));
        state.query().unwrap();
        state.complete(
            "target",
            page(
                vec![PageItem::Alias {
                    project_index: 1,
                    item_index: 0,
                    alias: "old".into(),
                }],
                Some("4"),
            ),
        );
        state.query().unwrap();
        state.complete("target", page(vec![location(1, 0, "/one")], Some("5")));
        assert!(state.ready());
        assert_eq!(state.rows[0].1.path, "/one");
        assert_eq!(state.preferred, Some(0));
        assert_eq!(state.next.as_deref(), Some("5"));
        state.previous.push_back(state.cursor.clone());
        state.cursor = state.next.clone();
        state.requested = true;
        state.query().unwrap();
        state.complete(
            "target",
            page(
                vec![
                    location(1, 1, "/two"),
                    project(2, "neighbor", 1),
                    location(2, 0, "/wrong"),
                ],
                Some("8"),
            ),
        );
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].1.path, "/two");
        assert!(state.next.is_none());
        state.cursor = state.previous.pop_back().unwrap();
        state.requested = true;
        assert!(matches!(state.query(),Some(Query::ListContinue {cursor,..}) if cursor=="4"));
        state.refresh();
        state.complete("target", page(vec![location(1, 0, "/obsolete")], None));
        assert!(!state.ready());
        assert!(matches!(state.query(), Some(Query::ListStart { .. })));
        state.complete(
            "target",
            page(
                vec![
                    project(0, "target", 1),
                    location(0, 0, "/current"),
                    project(1, "other", 1),
                    location(1, 0, "/not-target"),
                ],
                None,
            ),
        );
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].1.path, "/current");
        state.refresh();
        state.query().unwrap();
        state.complete("target", Err("offline".into()));
        assert!(state.query().is_none(), "failed reads do not spin");
        state.refresh();
        state.query().unwrap();
        state.complete("target", page(vec![project(0, "other", 0)], None));
        assert_eq!(state.error, Some("project-locations-missing"));
        state.refresh();
        state.query().unwrap();
        state.complete("target", page(vec![project(0, "target", 0)], None));
        assert!(state.ready() && state.rows.is_empty());
    }

    #[test]
    fn locations_modal_is_read_only_and_rejects_closed_generations_with_unicode_scrolling() {
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
            name: "项目".into(),
            entity: Entity::Project {
                id: "target".into(),
            },
        };
        let open = Action::Manage(Manage::Open(target, super::super::Kind::Locations));
        app.apply(open.clone());
        let old = app.locations_request().unwrap();
        app.apply(Action::Manage(Manage::Close));
        app.apply(open);
        assert!(app.locations_request().is_none());
        app.locations_completed(
            old,
            page(vec![project(0, "target", 1), location(0, 0, "/old")], None),
        );
        let request = app.locations_request().unwrap();
        app.locations_completed(
            request,
            page(
                vec![
                    project(0, "target", 1),
                    location(0, 0, &format!("/{}", "很长的目录 abc/".repeat(80))),
                ],
                None,
            ),
        );
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            for (width, height) in [(80, 24), (42, 13), (20, 8)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                assert!(
                    app.management_request().is_none(),
                    "a location view cannot become a mutation"
                );
                if width >= 42 {
                    app.input(Event::Key(crossterm::event::KeyEvent::new(
                        KeyCode::Home,
                        KeyModifiers::NONE,
                    )));
                    assert!(app.locations_enabled(&Command::Scroll(true)));
                    app.apply(Action::Manage(Manage::Locations(Command::Scroll(true))));
                    assert!(app.locations_enabled(&Command::Scroll(false)));
                } else {
                    assert!(!app.locations_enabled(&Command::Refresh));
                }
            }
        }
        app.project_catalog_changed();
        let request = app.locations_request().unwrap();
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "replacement".into(),
        };
        app.locations_completed(
            request,
            page(
                vec![project(0, "target", 1), location(0, 0, "/wrong-epoch")],
                None,
            ),
        );
        assert!(
            app.management
                .dialog
                .as_ref()
                .unwrap()
                .locations
                .as_ref()
                .unwrap()
                .rows
                .is_empty()
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.abandon_management();
        assert_eq!(
            app.management.dialog.as_ref().unwrap().error,
            Some("project-locations-disconnected")
        );
        assert!(app.locations_request().is_none());
        assert!(!app.locations_enabled(&Command::Refresh));
    }
}
