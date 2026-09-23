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

use crate::{
    app::{Action, App, ConnectionState, Focus, Hit},
    navigation::Route,
    view::{safe, tone},
};
use maka_protocol::session::{
    SessionCatalogProjection, SessionCatalogQueryInput, SessionCatalogQueryResult, SessionStatus,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

#[derive(Default)]
pub struct Sessions {
    pending_only: bool,
    pub items: Vec<SessionCatalogProjection>,
    pub selected: Option<String>,
    revision: Option<String>,
    cursor: Option<String>,
    next_cursor: Option<String>,
    previous: std::collections::VecDeque<Option<String>>,
    pub loading: bool,
    pub loaded: bool,
    requested: bool,
    pub error: Option<String>,
    pub changed: bool,
    pub detail: Detail,
    detail_generation: u64,
    detail_requested: bool,
    detail_inflight: bool,
}

#[derive(Default)]
pub enum Detail {
    #[default]
    Empty,
    Loading {
        id: String,
    },
    Ready(Box<SessionCatalogProjection>),
    Missing {
        id: String,
    },
    Failed {
        id: String,
        error: String,
    },
}
impl Detail {
    fn id(&self) -> Option<&str> {
        match self {
            Self::Empty => None,
            Self::Loading { id } | Self::Missing { id } | Self::Failed { id, .. } => Some(id),
            Self::Ready(item) => Some(&item.id),
        }
    }
}
#[derive(Clone)]
pub struct DetailRequest {
    pub id: String,
    generation: u64,
}

impl Sessions {
    pub fn inbox() -> Self {
        Self {
            pending_only: true,
            ..Self::default()
        }
    }
    pub fn refresh(&mut self) {
        self.changed = false;
        self.requested = true;
    }
    pub fn restart(&mut self) {
        if self.loading {
            return;
        }
        self.cursor = None;
        self.previous.clear();
        self.refresh();
    }
    pub fn can_next(&self) -> bool {
        !self.loading && self.next_cursor.is_some()
    }
    pub fn can_previous(&self) -> bool {
        !self.loading && !self.previous.is_empty()
    }
    pub fn can_refresh_detail(&self) -> bool {
        !self.detail_inflight && self.detail.id().is_some()
    }
    pub fn invalidate(&mut self, id: &str) {
        self.refresh();
        if self.detail.id() == Some(id) {
            self.refresh_detail();
        }
    }
    pub fn updated(&mut self, item: Box<SessionCatalogProjection>) {
        self.invalidate(&item.id);
        if let Some(current) = self.items.iter_mut().find(|current| current.id == item.id)
            && current.revision <= item.revision
        {
            *current = (*item).clone();
        }
        if self.detail.id() == Some(&item.id)
            && !matches!(&self.detail, Detail::Ready(current) if current.revision > item.revision)
        {
            self.detail = Detail::Ready(item);
        }
    }
    pub fn next(&mut self) {
        if self.loading {
            return;
        }
        if let Some(cursor) = self.next_cursor.clone() {
            self.changed = false;
            if self.previous.len() == 128 {
                self.previous.pop_front();
            }
            self.previous.push_back(self.cursor.clone());
            self.cursor = Some(cursor);
            self.requested = true;
        }
    }
    pub fn previous(&mut self) {
        if self.loading {
            return;
        }
        if let Some(cursor) = self.previous.pop_back() {
            self.changed = false;
            self.cursor = cursor;
            self.requested = true;
        }
    }
    pub fn query(&mut self) -> Option<SessionCatalogQueryInput> {
        if self.loading || !self.requested {
            return None;
        }
        self.requested = false;
        self.loading = true;
        self.error = None;
        Some(match (&self.revision, &self.cursor) {
            (Some(revision), Some(cursor)) if self.pending_only => {
                SessionCatalogQueryInput::PendingContinue {
                    revision: revision.clone(),
                    cursor: cursor.clone(),
                }
            }
            _ if self.pending_only => SessionCatalogQueryInput::PendingStart,
            (Some(revision), Some(cursor)) => SessionCatalogQueryInput::ListContinue {
                revision: revision.clone(),
                cursor: cursor.clone(),
            },
            _ => SessionCatalogQueryInput::ListStart,
        })
    }
    pub fn complete(&mut self, result: Result<SessionCatalogQueryResult, String>) {
        self.loading = false;
        match result {
            Ok(SessionCatalogQueryResult::Page {
                revision,
                sessions,
                next_cursor,
            }) => {
                if !self.loaded {
                    self.selected = sessions.first().map(|item| item.id.clone());
                } else if !sessions
                    .iter()
                    .any(|item| Some(&item.id) == self.selected.as_ref())
                {
                    // A removed/reordered row must not silently select another entity.
                    self.selected = None;
                }
                self.items = sessions;
                self.revision = Some(revision);
                self.next_cursor = next_cursor;
                self.loaded = true;
            }
            Ok(SessionCatalogQueryResult::RevisionChanged { .. }) => {
                self.cursor = None;
                self.previous.clear();
                self.changed = true;
                self.requested = true; // One fresh list_start; the client rejects revision_changed for it.
            }
            Err(error) => self.error = Some(error),
            _ => unreachable!("client checks catalog reply variants"),
        }
    }
    pub fn move_selection(&mut self, down: bool) {
        let index = self
            .items
            .iter()
            .position(|item| Some(&item.id) == self.selected.as_ref());
        let next = index.map_or(0, |i| {
            if down {
                (i + 1).min(self.items.len().saturating_sub(1))
            } else {
                i.saturating_sub(1)
            }
        });
        self.selected = self.items.get(next).map(|item| item.id.clone());
    }
    pub fn open(&mut self, id: &str) {
        if self.detail.id() == Some(id) {
            return;
        }
        self.detail = Detail::Loading { id: id.into() };
        self.refresh_detail();
    }
    pub fn refresh_detail(&mut self) {
        self.detail_generation += 1;
        self.detail_requested = true;
    }
    pub fn detail_query(&mut self) -> Option<DetailRequest> {
        if self.detail_inflight || !self.detail_requested {
            return None;
        }
        let id = self.detail.id()?.to_owned();
        self.detail_requested = false;
        self.detail_inflight = true;
        Some(DetailRequest {
            id,
            generation: self.detail_generation,
        })
    }
    pub fn complete_detail(
        &mut self,
        request: DetailRequest,
        result: Result<Option<Box<SessionCatalogProjection>>, String>,
    ) {
        self.detail_inflight = false;
        if request.generation != self.detail_generation || self.detail.id() != Some(&request.id) {
            return;
        }
        self.detail = match result {
            Ok(Some(item)) => Detail::Ready(item),
            Ok(None) => Detail::Missing { id: request.id },
            Err(error) => Detail::Failed {
                id: request.id,
                error,
            },
        };
    }
}

pub fn draw_catalog(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let area = area.inner(ratatui::layout::Margin::new(
        1,
        u16::from(area.height >= 18),
    ));
    let connected = matches!(app.connection, ConnectionState::Connected { .. });
    let state = if app.navigation.current() == Route::Inbox {
        &app.inbox
    } else {
        &app.sessions
    };
    let message = if !connected {
        "workspace-connect"
    } else if state.loading && state.items.is_empty() {
        "sessions-loading"
    } else if state.error.is_some() {
        if state.pending_only {
            "inbox-failed"
        } else {
            "sessions-failed"
        }
    } else if state.changed && state.items.is_empty() {
        "sessions-changed"
    } else if state.items.is_empty() {
        if state.pending_only {
            "inbox-empty"
        } else {
            "sessions-empty"
        }
    } else {
        "sessions-help"
    };
    let heading_rows = if message == "sessions-help" {
        0
    } else if state.error.is_some() && !state.pending_only {
        3
    } else {
        2
    };
    let parts =
        Layout::vertical([Constraint::Length(heading_rows), Constraint::Min(0)]).split(area);
    let heading = if let Some(error) = &state.error
        && !state.pending_only
    {
        format!("{}\n{}", app.i18n.text(message), safe(error))
    } else {
        app.i18n.text(message)
    };
    frame.render_widget(Paragraph::new(heading).wrap(Wrap { trim: false }), parts[0]);
    let stride = if area.height >= 18 { 3 } else { 2 };
    let visible = parts[1].height as usize / stride;
    let selected = state
        .items
        .iter()
        .position(|item| Some(&item.id) == state.selected.as_ref());
    let offset = selected.map_or(0, |index| (index + 1).saturating_sub(visible));
    for (index, item) in state.items.iter().enumerate().skip(offset).take(visible) {
        let rect = Rect::new(
            parts[1].x,
            parts[1].y + ((index - offset) * stride) as u16,
            parts[1].width,
            2,
        );
        let action = Action::Visit(Route::Session(item.id.clone()));
        let active = app.palette.is_none()
            && (app.focus == Focus::List && selected == Some(index)
                || app.hover.as_ref() == Some(&action));
        let style = if active {
            tone::selection(app.theme.colors())
        } else {
            Style::default()
        };
        let working = connected
            && item.status != SessionStatus::WaitingForUser
            && (item.status == SessionStatus::Running
                || item
                    .live_run_state
                    .as_ref()
                    .is_some_and(|run| !run.running_turn_ids.is_empty()));
        let orbit = if working {
            app.chrome
                .animation
                .frame(crate::motion::Loop::Orbit, app.chrome.ascii)
        } else {
            "  "
        };
        let mut title = vec![
            Span::styled(
                if active {
                    app.chrome.symbol("› ", "> ")
                } else {
                    "  "
                },
                Style::default().fg(tone::accent(app.theme.colors())),
            ),
            Span::styled(orbit, Style::default().fg(tone::accent(app.theme.colors()))),
            Span::raw(" "),
            Span::styled(
                safe(&item.name),
                Style::default()
                    .fg(tone::session(&item.id, app.theme.colors()))
                    .add_modifier(if active || item.has_unread {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ];
        if item.status != SessionStatus::Active && !working && !state.pending_only {
            title.push(Span::raw(format!(
                "  {}",
                app.i18n.text(status_key(item.status))
            )));
        }
        if item.is_archived {
            title.push(Span::raw(format!(
                " · {}",
                app.i18n.text("session-archived")
            )));
        }
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(title),
                Line::styled(
                    format!(
                        "     {} · {}",
                        safe(&item.workspace.host_cwd),
                        safe(&item.model)
                    ),
                    Style::default().fg(tone::secondary(app.theme.colors())),
                ),
            ])
            .style(style),
            rect,
        );
        app.hits.push(Hit { area: rect, action });
    }
}

pub fn detail_lines(app: &App) -> Vec<Line<'static>> {
    let i18n = &app.i18n;
    match &app.sessions.detail {
        Detail::Empty | Detail::Loading { .. } => vec![Line::raw(i18n.text("sessions-loading"))],
        Detail::Missing { id } => {
            vec![Line::raw(i18n.text("session-missing")), Line::raw(safe(id))]
        }
        Detail::Failed { error, .. } => vec![
            Line::raw(i18n.text("sessions-failed")),
            Line::raw(safe(error)),
        ],
        Detail::Ready(item) => vec![
            Line::raw(safe(&item.name)),
            Line::raw(""),
            Line::raw(i18n.format("session-id", &[("value", &safe(&item.id))])),
            Line::raw(i18n.text(status_key(item.status))),
            Line::raw(i18n.format(
                "session-workspace",
                &[("value", &safe(&item.workspace.host_cwd))],
            )),
            Line::raw(i18n.format("session-model", &[("value", &safe(&item.model))])),
            Line::raw(""),
            Line::raw(safe(item.last_message_preview.as_deref().unwrap_or(""))),
        ],
    }
}

fn status_key(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "session-active",
        SessionStatus::Running => "session-running",
        SessionStatus::WaitingForUser => "session-waiting",
        SessionStatus::Blocked => "session-blocked",
        SessionStatus::Aborted => "session-aborted",
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn item(id: &str) -> SessionCatalogProjection {
        maka_protocol::session::decode_session_catalog_projection(&serde_json::json!({
            "id":id,"revision":1,"workspace":{"target":{"kind":"host_path","path":"/work"},"hostCwd":"/work"},
            "createdAt":0,"activityAt":1,"name":id,"isFlagged":false,"isArchived":false,
            "labels":[],"labelsTruncated":false,"hasUnread":false,"status":"active","backend":"ai-sdk",
            "llmConnectionId":null,"llmConnectionSlug":"default","connectionLocked":false,"model":"model",
            "sandboxMode":"workspace-write","approvalPolicy":{"kind":"on-request"},"collaborationMode":"agent","orchestrationMode":"default"
        })).unwrap()
    }
    #[test]
    fn notices_during_catalog_load_are_not_lost_and_revision_change_restarts_once() {
        for mut state in [Sessions::default(), Sessions::inbox()] {
            let start = if state.pending_only {
                SessionCatalogQueryInput::PendingStart
            } else {
                SessionCatalogQueryInput::ListStart
            };
            state.refresh();
            assert_eq!(state.query(), Some(start.clone()));
            state.invalidate("A");
            assert!(state.query().is_none());
            state.complete(Ok(SessionCatalogQueryResult::Page {
                revision: "one".into(),
                sessions: vec![item("A")],
                next_cursor: Some("cursor".into()),
            }));
            assert!(
                state.query().is_some(),
                "in-flight invalidation schedules another query"
            );
            state.complete(Ok(SessionCatalogQueryResult::Page {
                revision: "one".into(),
                sessions: vec![item("A")],
                next_cursor: Some("cursor".into()),
            }));
            state.next();
            let continuation = state.query().unwrap();
            assert_eq!(
                matches!(
                    continuation,
                    SessionCatalogQueryInput::PendingContinue { .. }
                ),
                state.pending_only
            );
            state.complete(Ok(SessionCatalogQueryResult::RevisionChanged {
                expected_revision: "one".into(),
                actual_revision: "two".into(),
            }));
            assert_eq!(state.query(), Some(start.clone()));
            assert!(state.previous.is_empty());
            assert!(state.changed);
            state.complete(Err("offline".into()));
            assert!(state.query().is_none(), "failed reads do not spin");
            for index in 0..130 {
                state.complete(Ok(SessionCatalogQueryResult::Page {
                    revision: "one".into(),
                    sessions: vec![item("A")],
                    next_cursor: Some(format!("cursor-{index}")),
                }));
                assert!(
                    state.can_next(),
                    "bounded Back history must not hide more pages"
                );
                state.next();
                assert!(state.query().is_some());
            }
            assert_eq!(state.previous.len(), 128);
            state.complete(Ok(SessionCatalogQueryResult::Page {
                revision: "one".into(),
                sessions: vec![],
                next_cursor: None,
            }));
            state.restart();
            assert_eq!(state.query(), Some(start));
            assert!(state.previous.is_empty());
        }
    }

    #[test]
    fn late_a_result_cannot_overwrite_b_or_a_reopened_after_b() {
        let mut state = Sessions::default();
        state.open("A");
        let old = state.detail_query().unwrap();
        state.open("B");
        state.open("A");
        assert!(
            state.detail_query().is_none(),
            "only one detail request can be in flight"
        );
        state.complete_detail(old, Ok(None));
        assert!(matches!(state.detail, Detail::Loading { .. }));
        let current = state.detail_query().unwrap();
        state.complete_detail(current, Ok(None));
        assert!(matches!(state.detail, Detail::Missing { ref id } if id == "A"));
        let mut acknowledged = item("A");
        acknowledged.revision = 9;
        acknowledged.is_archived = true;
        state.updated(Box::new(acknowledged));
        state.updated(Box::new(item("A"))); // A late mutation ack cannot roll back the visible revision.
        assert!(
            matches!(&state.detail, Detail::Ready(item) if item.revision == 9 && item.is_archived)
        );
    }

    #[test]
    fn catalog_selection_tracks_identity_across_reorder_and_clears_on_removal() {
        let mut state = Sessions::default();
        let page = |ids: &[&str]| {
            Ok(SessionCatalogQueryResult::Page {
                revision: format!("sha256:{}", "a".repeat(64)),
                sessions: ids.iter().map(|id| item(id)).collect(),
                next_cursor: None,
            })
        };
        state.complete(page(&["A", "B"]));
        state.move_selection(true);
        assert_eq!(state.selected.as_deref(), Some("B"));
        state.complete(page(&["B", "A"]));
        assert_eq!(state.selected.as_deref(), Some("B"));
        state.open("B");
        state.complete(page(&["A"]));
        assert_eq!(state.selected, None);
        assert_eq!(
            state.detail.id(),
            Some("B"),
            "removing a row must not retarget the open route"
        );
        let mut app = App::new(
            "/unused".into(),
            crate::i18n::I18n::new(
                crate::LocalePreference::Explicit(crate::Locale::En),
                crate::Locale::En,
            ),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Visit(Route::Session("draft".into())));
        app.drafts.get_mut("draft").unwrap().insert("Keep typing");
        app.apply(Action::ToggleFullscreen);
        app.inbox.complete(page(&["A", "B"]));
        assert_eq!(app.focus, Focus::Composer);
        assert!(app.page_actions().contains(&Action::Visit(Route::Inbox)));
        assert!(!app.interactions.visible);
        app.apply(Action::Visit(Route::Inbox));
        assert_eq!(app.focus, Focus::List);
        assert_eq!(app.inbox.selected.as_deref(), Some("A"));
        let mut screen =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        screen
            .draw(|frame| crate::view::draw(frame, &mut app))
            .unwrap();
        let row = app
            .hits
            .iter()
            .find(|hit| hit.action == Action::Visit(Route::Session("A".into())))
            .unwrap()
            .area;
        app.inbox.refresh();
        assert!(app.inbox.query().is_some());
        screen
            .draw(|frame| crate::view::draw(frame, &mut app))
            .unwrap();
        assert_eq!(
            app.hits
                .iter()
                .find(|hit| hit.action == Action::Visit(Route::Session("A".into())))
                .unwrap()
                .area,
            row,
            "background refresh must not move visible rows under the pointer"
        );
        app.inbox.complete(page(&["B"]));
        let key = |code| {
            crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
                code,
                crossterm::event::KeyModifiers::NONE,
            ))
        };
        app.input(key(crossterm::event::KeyCode::Enter));
        assert_eq!(
            app.navigation.current(),
            Route::Inbox,
            "resolved A cannot silently activate B"
        );
        app.input(key(crossterm::event::KeyCode::Down));
        app.input(key(crossterm::event::KeyCode::Enter));
        assert_eq!(app.navigation.current(), Route::Session("B".into()));
        assert!(
            !app.management_commands().is_empty(),
            "catalog identity enables commands while session detail is loading"
        );
        assert_eq!(app.drafts["draft"].text(), "Keep typing");
    }
}
