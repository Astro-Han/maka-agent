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

use super::{Command as Manage, Target};
use crate::app::{Action, App};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use maka_protocol::project::{DIRECTORY_MAX_SEGMENTS, Query, QueryResult};
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Open(usize),
    Path,
    Parent,
    Refresh,
    Previous,
    Next,
}
impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Open(_) => "directory-open",
            Self::Path => "directory-path",
            Self::Parent => "directory-parent",
            Self::Refresh => "command-refresh",
            Self::Previous => "sessions-previous",
            Self::Next => "sessions-next",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Location {
    pub root_id: String,
    pub segments: Vec<String>,
    label: String,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    generation: u64,
    target: Target,
    pub query: Query,
}
struct Row {
    name: String,
    location: Location,
}
pub(super) struct Browser {
    generation: u64,
    pub location: Option<Location>,
    rows: Vec<Row>,
    selected: usize,
    focus: usize, // List, path, parent, refresh, previous, next, cancel, register.
    hovered: Option<Manage>,
    requested: bool,
    loading: bool,
    error: bool,
    cursor: Option<String>,
    next: Option<String>,
    previous: VecDeque<Option<String>>,
}
impl Browser {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            location: None,
            rows: vec![],
            selected: 0,
            focus: 0,
            hovered: None,
            requested: true,
            loading: false,
            error: false,
            cursor: None,
            next: None,
            previous: VecDeque::new(),
        }
    }
    fn ready(&self) -> bool {
        !self.requested && !self.loading && !self.error
    }
    pub fn can_register(&self) -> bool {
        self.ready() && self.location.is_some()
    }
    pub fn invalidate_geometry(&mut self) {
        self.hovered = None;
    }
    fn reset(&mut self) {
        self.rows.clear();
        self.selected = 0;
        self.next = None;
        self.hovered = None;
        self.error = false;
        self.requested = true;
    }
    fn navigate(&mut self, location: Option<Location>) {
        self.location = location;
        self.cursor = None;
        self.previous.clear();
        self.focus = 0;
        self.reset();
    }
    fn query(&self) -> Query {
        match &self.location {
            None => Query::DirectoryRoots,
            Some(location) => match &self.cursor {
                None => Query::DirectoryListStart {
                    root_id: location.root_id.clone(),
                    segments: location.segments.clone(),
                },
                Some(cursor) => Query::DirectoryListContinue {
                    root_id: location.root_id.clone(),
                    segments: location.segments.clone(),
                    cursor: cursor.clone(),
                },
            },
        }
    }
    fn move_selection(&mut self, down: bool) {
        self.focus = 0;
        self.selected = if down {
            (self.selected + 1).min(self.rows.len().saturating_sub(1))
        } else {
            self.selected.saturating_sub(1)
        };
    }
}

impl App {
    pub(super) fn directory_enabled(&self, command: &Command) -> bool {
        let Some(dialog) = self.management.dialog.as_ref() else {
            return false;
        };
        let Some(browser) = &dialog.browser else {
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
            Command::Path => true,
            Command::Parent => !browser.loading && browser.location.is_some(),
            Command::Refresh => !browser.loading && !browser.requested,
            Command::Previous => {
                !browser.loading && !browser.requested && !browser.previous.is_empty()
            }
            Command::Next => browser.ready() && browser.next.is_some(),
            Command::Open(index) => {
                browser.ready()
                    && browser
                        .rows
                        .get(*index)
                        .is_some_and(|row| row.location.segments.len() <= DIRECTORY_MAX_SEGMENTS)
            }
        }
    }
    pub(super) fn directory_action(&mut self, command: Command) -> Option<Action> {
        let dialog = self.management.dialog.as_mut()?;
        let browser = dialog.browser.as_mut()?;
        match command {
            Command::Path => {
                dialog.browser = None;
                dialog.focus = 1;
            }
            Command::Open(index) => {
                let location = browser.rows.get(index)?.location.clone();
                browser.navigate(Some(location));
            }
            Command::Parent => {
                let mut parent = browser.location.clone()?;
                let has_parent = parent.segments.pop().is_some();
                browser.navigate(has_parent.then_some(parent));
            }
            Command::Refresh => {
                browser.cursor = None;
                browser.previous.clear();
                browser.reset();
            }
            Command::Previous => {
                browser.cursor = browser.previous.pop_back()?;
                browser.reset();
            }
            Command::Next => {
                let next = browser.next.take()?;
                browser.previous.push_back(browser.cursor.replace(next));
                if browser.previous.len() > 128 {
                    browser.previous.pop_front();
                }
                browser.reset();
            }
        }
        dialog.error = None;
        dialog.visible = false;
        self.hits.clear();
        None
    }
    pub fn directory_request(&mut self) -> Option<Request> {
        if self.management.directory_pending.is_some() {
            return None;
        }
        let dialog = self.management.dialog.as_ref()?;
        if dialog.blocked || !self.management_identity(&dialog.target) {
            return None;
        }
        let dialog = self.management.dialog.as_mut()?;
        let browser = dialog.browser.as_mut()?;
        if !browser.requested || browser.loading {
            return None;
        }
        browser.requested = false;
        browser.loading = true;
        let request = Request {
            generation: browser.generation,
            target: dialog.target.clone(),
            query: browser.query(),
        };
        self.management.directory_pending = Some(request.clone());
        Some(request)
    }
    pub fn directory_completed(&mut self, request: Request, result: Result<QueryResult, String>) {
        if self.management.directory_pending.as_ref() != Some(&request) {
            return;
        }
        self.management.directory_pending = None;
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
        let Some(browser) = dialog
            .browser
            .as_mut()
            .filter(|b| b.generation == request.generation)
        else {
            return;
        };
        browser.loading = false;
        match result {
            Ok(QueryResult::DirectoryRoots { roots }) => {
                browser.rows = roots
                    .into_iter()
                    .map(|root| Row {
                        name: root.label.clone(),
                        location: Location {
                            root_id: root.id,
                            label: root.label,
                            segments: vec![],
                        },
                    })
                    .collect();
            }
            Ok(QueryResult::DirectoryPage {
                entries,
                next_cursor,
                ..
            }) => {
                let Some(location) = &browser.location else {
                    return;
                };
                browser.rows = entries
                    .into_iter()
                    .map(|entry| {
                        let mut location = location.clone();
                        location.segments.push(entry.name.clone());
                        Row {
                            name: entry.name,
                            location,
                        }
                    })
                    .collect();
                browser.next = next_cursor;
            }
            _ => browser.error = true,
        }
    }
    pub(super) fn directory_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let dialog = self.management.dialog.as_mut().expect("directory dialog");
        let browser = dialog.browser.as_mut().expect("directory browser");
        if matches!(&event, Event::Key(key) if key.kind != KeyEventKind::Release) {
            browser.hovered = None;
        }
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                KeyCode::Esc => Some(
                    if !dialog.visible || dialog.blocked || self.management.pending.is_some() {
                        Manage::Close
                    } else {
                        Manage::Directory(Command::Path)
                    },
                ),
                _ if !dialog.visible => return (false, None),
                KeyCode::Tab => {
                    browser.focus = (browser.focus + 1) % 8;
                    return (true, None);
                }
                KeyCode::BackTab => {
                    browser.focus = (browser.focus + 7) % 8;
                    return (true, None);
                }
                KeyCode::Up | KeyCode::Down => {
                    browser.move_selection(key.code == KeyCode::Down);
                    return (true, None);
                }
                KeyCode::Home => {
                    browser.focus = 0;
                    browser.selected = 0;
                    return (true, None);
                }
                KeyCode::End => {
                    browser.focus = 0;
                    browser.selected = browser.rows.len().saturating_sub(1);
                    return (true, None);
                }
                KeyCode::Backspace | KeyCode::Left => Some(Manage::Directory(Command::Parent)),
                KeyCode::F(5) => Some(Manage::Directory(Command::Refresh)),
                KeyCode::PageDown => Some(Manage::Directory(Command::Next)),
                KeyCode::PageUp => Some(Manage::Directory(Command::Previous)),
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(Manage::Save)
                }
                KeyCode::Right if browser.focus == 0 => {
                    Some(Manage::Directory(Command::Open(browser.selected)))
                }
                KeyCode::Enter => Some(focused(browser)),
                _ => None,
            },
            Event::Mouse(mouse) if dialog.visible => {
                let hit = self
                    .hits
                    .iter()
                    .rev()
                    .find(|hit| hit.area.contains((mouse.column, mouse.row).into()))
                    .and_then(|hit| match &hit.action {
                        Action::Manage(command) => Some(command.clone()),
                        _ => None,
                    });
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => hit,
                    MouseEventKind::Moved => {
                        let changed = browser.hovered != hit;
                        browser.hovered = hit;
                        return (changed, None);
                    }
                    MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                        if matches!(hit, Some(Manage::Directory(Command::Open(_)))) =>
                    {
                        browser.move_selection(mouse.kind == MouseEventKind::ScrollDown);
                        return (true, None);
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        (
            command.is_some(),
            command.and_then(|command| self.apply(Action::Manage(command))),
        )
    }
}

fn focused(browser: &Browser) -> Manage {
    match browser.focus {
        0 => Manage::Directory(Command::Open(browser.selected)),
        1 => Manage::Directory(Command::Path),
        2 => Manage::Directory(Command::Parent),
        3 => Manage::Directory(Command::Refresh),
        4 => Manage::Directory(Command::Previous),
        5 => Manage::Directory(Command::Next),
        6 => Manage::Close,
        _ => Manage::Save,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Locale, LocalePreference, app::ConnectionState, i18n::I18n};
    use crossterm::event::KeyEvent;
    use maka_protocol::project::{DirectoryEntry, DirectoryRoot};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn directory_picker_bounds_reads_and_keeps_host_identity_across_modal_lifetimes() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "host".into(),
            epoch: "epoch".into(),
        };
        let render = |app: &mut App, width, height| {
            Terminal::new(TestBackend::new(width, height))
                .unwrap()
                .draw(|f| crate::view::draw(f, app))
                .unwrap();
        };
        let apply = |app: &mut App, command| {
            app.apply(Action::Manage(command));
        };
        let roots = || {
            Ok(QueryResult::DirectoryRoots {
                roots: vec![DirectoryRoot {
                    id: "opaque-root".into(),
                    label: "不是路径".into(),
                }],
            })
        };
        let page = |segments, entries: &[&str], next: Option<&str>| {
            Ok(QueryResult::DirectoryPage {
                root_id: "opaque-root".into(),
                segments,
                entries: entries
                    .iter()
                    .map(|name| DirectoryEntry {
                        name: (*name).into(),
                    })
                    .collect(),
                next_cursor: next.map(String::from),
            })
        };
        app.apply(app.register_project_action().unwrap());
        render(&mut app, 80, 24);
        app.input(Event::Paste("/manual draft".into()));
        apply(&mut app, Manage::Browse);
        let stale = app.directory_request().unwrap();
        assert!(app.directory_request().is_none());
        render(&mut app, 80, 24);
        apply(&mut app, Manage::Directory(Command::Path));
        assert_eq!(
            app.management.dialog.as_ref().unwrap().editor.text(),
            "/manual draft"
        );
        render(&mut app, 80, 24);
        apply(&mut app, Manage::Browse);
        assert!(
            app.directory_request().is_none(),
            "closed reads still occupy the single slot"
        );
        app.directory_completed(stale.clone(), roots());
        assert!(
            app.management
                .dialog
                .as_ref()
                .unwrap()
                .browser
                .as_ref()
                .unwrap()
                .rows
                .is_empty()
        );
        let request = app.directory_request().unwrap();
        app.directory_completed(stale, roots());
        assert_eq!(app.management.directory_pending.as_ref(), Some(&request));
        app.directory_completed(request, roots());
        render(&mut app, 80, 24);
        assert!(
            !app.management_enabled(&Manage::Save),
            "roots overview is not a registration target"
        );
        apply(&mut app, Manage::Directory(Command::Open(0)));
        let request = app.directory_request().unwrap();
        assert!(
            matches!(&request.query, Query::DirectoryListStart {root_id,segments} if root_id == "opaque-root" && segments.is_empty())
        );
        render(&mut app, 80, 24);
        assert!(
            !app.management_enabled(&Manage::Save),
            "unconfirmed directories cannot register"
        );
        app.directory_completed(request, page(vec![], &["中文", "🦀"], Some("🦀")));
        render(&mut app, 80, 24);
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert!(
            app.management.pending.is_none(),
            "Enter opens a child; it never registers from the list"
        );
        let request = app.directory_request().unwrap();
        assert!(
            matches!(&request.query, Query::DirectoryListStart {root_id,segments} if root_id == "opaque-root" && segments == &["中文"])
        );
        app.directory_completed(request, Err("filesystem unavailable".into()));
        render(&mut app, 80, 24);
        assert!(!app.management_enabled(&Manage::Save));
        assert!(
            app.directory_request().is_none(),
            "errors wait for explicit recovery"
        );
        apply(&mut app, Manage::Directory(Command::Parent));
        let request = app.directory_request().unwrap();
        app.directory_completed(request, page(vec![], &["a"], Some("a")));
        for index in 0..130 {
            render(&mut app, 80, 24);
            apply(&mut app, Manage::Directory(Command::Next));
            let request = app.directory_request().unwrap();
            let name = format!("next-{index}");
            app.directory_completed(request, page(vec![], &[&name], Some(&name)));
        }
        let browser = app
            .management
            .dialog
            .as_ref()
            .unwrap()
            .browser
            .as_ref()
            .unwrap();
        assert_eq!(browser.previous.len(), 128);
        assert!(
            browser.next.is_some(),
            "history bound is not a forward paging limit"
        );
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            for (width, height) in [(80, 24), (52, 22)] {
                render(&mut app, width, height);
                assert!(app.management_enabled(&Manage::Save));
                assert!(
                    app.hits
                        .iter()
                        .any(|h| h.action == Action::Manage(Manage::Save))
                );
            }
        }
        app.management
            .dialog
            .as_mut()
            .unwrap()
            .browser
            .as_mut()
            .unwrap()
            .hovered = Some(Manage::Directory(Command::Parent));
        app.input(Event::Resize(40, 16));
        assert!(
            app.management
                .dialog
                .as_ref()
                .unwrap()
                .browser
                .as_ref()
                .unwrap()
                .hovered
                .is_none(),
            "resize cannot retain a tooltip tied to an old hit region"
        );
        render(&mut app, 40, 16);
        assert!(!app.management_enabled(&Manage::Save));
        render(&mut app, 80, 24);
        let ticket = app.management_request().unwrap();
        assert_eq!(ticket.directory.as_ref().unwrap().root_id, "opaque-root");
        assert!(ticket.directory.as_ref().unwrap().segments.is_empty());
        assert!(
            !app.directory_enabled(&Command::Path),
            "in-flight registration cannot switch targets"
        );
        app.management_completed(
            ticket,
            Err(maka_client::RequestFailure::Unknown(
                maka_client::ClientError::Timeout,
            )),
        );
        assert!(!app.management_enabled(&Manage::Save));
        assert!(
            !app.directory_enabled(&Command::Path),
            "unknown writes cannot be replayed via the path editor"
        );
        assert!(app.directory_request().is_none());
        app.abandon_management();
        assert_eq!(
            app.management.dialog.as_ref().unwrap().error,
            Some("session-edit-unknown")
        );
    }
}
