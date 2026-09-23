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

use super::*;
use maka_protocol::{session::SessionStatus, turn::TurnState};
use ratatui::{
    text::Span,
    widgets::{BorderType, Padding},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

mod feedback;
mod search;

/// One reading surface, one input surface. Metadata lives on the input boundary.
pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, id: &str) {
    let extras = app.stop_target().is_some() && app.enabled(&Action::SendMessage);
    let control_width = if extras { 9 } else { 3 };
    let width = area.width.saturating_sub(4 + control_width).max(1);
    let editor_height = app.drafts.get_mut(id).map_or(1, |editor| {
        editor.preferred_height(width, (area.height / 3).clamp(1, 8))
    });
    let parts = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(super::queue::height(app, area.height)),
        Constraint::Length(1),
        Constraint::Length(editor_height + 2),
    ])
    .split(area);
    if app.chrome.details {
        let mut lines = crate::pages::sessions::detail_lines(app);
        if let Detail::Ready(item) = &app.sessions.detail {
            lines.push(Line::raw(app.i18n.text(sandbox_key(item.sandbox_mode))));
        }
        if let Some(context) = app.chat.context.label(&app.i18n) {
            lines.push(Line::raw(context));
        }
        lines.extend(feedback::details(app).into_iter().map(Line::raw));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), parts[0]);
    } else {
        app.chat.view.colors = app.theme.colors();
        app.chat.view.focused = app.focus == Focus::Transcript
            && !app.chat.view.mouse_selected
            && !app.chat.view.text_selection.active()
            && !app.chat.view.text_selection.dragging()
            && app.palette.is_none()
            && !app.interactions.visible
            && app.management.dialog.is_none()
            && app.chat.view.search.is_none();
        app.chat.view.hovered = match &app.hover {
            Some(Action::ToggleMessage(key)) => Some(key.clone()),
            _ => None,
        };
        let search_height = u16::from(app.chat.view.search.is_some());
        let body = Rect::new(
            parts[0].x,
            parts[0].y + search_height + u16::from(parts[0].height > 4),
            parts[0].width,
            parts[0]
                .height
                .saturating_sub(search_height + u16::from(parts[0].height > 4)),
        );
        if !search::body(frame, app, body) {
            app.hits
                .extend(app.chat.draw(frame, body, &app.i18n, app.chrome.ascii));
        }
        search::draw(
            frame,
            app,
            Rect::new(parts[0].x, parts[0].y, parts[0].width, search_height),
        );
        if app.chrome.window_focused
            && app
                .chat
                .reader()
                .is_some_and(|reader| reader.timing_visible())
        {
            app.chrome
                .animation
                .wake_after(std::time::Duration::from_secs(1));
        }
        if app.chrome.window_focused
            && let Some(wait) = app.chat.stream_wait()
        {
            app.chrome.animation.wake_after(wait);
        }
    }
    super::queue::draw(frame, app, parts[1]);
    let feedback = feedback::current(app);
    let status = feedback
        .as_ref()
        .map_or_else(|| activity(app), |item| app.i18n.text(item.key));
    let gap = parts[2];
    let count = app.queue_rows().len();
    let reserved = if count > 0 {
        format!(" ≡ {count} ").width() as u16
    } else {
        0
    };
    let has_details = feedback.as_ref().is_some_and(|item| item.detail.is_some());
    let notice_area = Rect::new(gap.x, gap.y, gap.width.saturating_sub(reserved), gap.height);
    let summary = fit(
        &status,
        usize::from(
            notice_area
                .width
                .saturating_sub(if has_details { 4 } else { 0 }),
        ),
    );
    let summary = if has_details {
        format!("{summary}  {}", app.chrome.symbol("ⓘ", "i"))
    } else {
        summary
    };
    frame.render_widget(
        Paragraph::new(summary)
            .centered()
            .style(
                Style::default().fg(if feedback.as_ref().is_some_and(|item| item.warning) {
                    app.theme.colors().warning
                } else {
                    app.theme.colors().muted
                }),
            ),
        notice_area,
    );
    if has_details && !notice_area.is_empty() {
        app.hits.push(Hit {
            area: notice_area,
            action: Action::ToggleDetails,
        });
    }
    if count > 0 {
        let title = format!(" {} {count} ", app.chrome.symbol("≡", "Q"));
        let width = title.width() as u16;
        button(
            frame,
            app,
            Rect::new(gap.right().saturating_sub(width), gap.y, width, 1),
            &title,
            Action::Queue(crate::pages::queue::Command::Focus),
            false,
        );
    }

    let focused = app.focus == Focus::Composer
        && app.chat.view.search.is_none()
        && app.palette.is_none()
        && !app.interactions.visible
        && app.management.dialog.is_none()
        && app.onboarding.dialog.is_none()
        && app.queue.edit.is_none();
    let (mut metadata, model_width) = metadata(app, parts[3].width.saturating_sub(4));
    let model_action = app.model_action();
    if model_width > 0
        && model_action
            .as_ref()
            .is_some_and(|action| app.hover.as_ref() == Some(action))
    {
        metadata.spans[1].style = Style::default().fg(app.theme.colors().accent);
    }
    let metadata_width = metadata.width() as u16;
    let metadata_area = Rect::new(
        parts[3].right().saturating_sub(metadata_width + 4),
        parts[3].bottom().saturating_sub(1),
        metadata_width,
        1,
    );
    let working = app.session_activity(id) == super::activity::Activity::Working;
    let breath = (working && (app.theme.choice != crate::theme::Choice::Terminal))
        .then(|| app.chrome.animation.breath());
    let border = Block::bordered()
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .padding(Padding::new(2, control_width, 0, 0))
        .border_style(Style::default().fg(tone::border(app.theme.colors(), focused, breath)));
    let input = border.inner(parts[3]);
    frame.render_widget(border, parts[3]);
    frame.render_widget(Paragraph::new(metadata), metadata_area);
    if model_width > 0
        && let Some(action) = model_action
        && app.enabled(&action)
    {
        app.hits.push(Hit {
            area: Rect::new(metadata_area.x + 1, metadata_area.y, model_width, 1),
            action,
        });
    }
    if input.is_empty() {
        app.invalidate_editor_geometry();
        return;
    }
    frame.render_widget(
        Paragraph::new(app.chrome.symbol("›", ">"))
            .style(Style::default().fg(app.theme.colors().muted)),
        Rect::new(parts[3].x + 1, input.y, 1, 1),
    );
    if let Some(editor) = app.drafts.get_mut(id) {
        editor.draw(frame, input, focused, app.theme.colors());
        if editor.text().is_empty() {
            frame.render_widget(
                Paragraph::new(app.i18n.text("composer-placeholder"))
                    .style(Style::default().fg(app.theme.colors().subtle)),
                input,
            );
        }
    } else {
        frame.render_widget(
            Paragraph::new(app.i18n.text("composer-draft-limit")).wrap(Wrap { trim: false }),
            input,
        );
    }
    let action = app.send_action();
    let selected =
        app.focus == Focus::Page && app.page_actions().get(app.selected_control) == Some(&action);
    icon_button(
        frame,
        app,
        Rect::new(parts[3].right() - 4, input.y, 3, 1),
        action,
        selected,
    );
    if extras {
        for (offset, action) in [(10, Action::SendMessage), (7, Action::SteerMessage)] {
            let selected = app.focus == Focus::Page
                && app.page_actions().get(app.selected_control) == Some(&action);
            icon_button(
                frame,
                app,
                Rect::new(parts[3].right() - offset, input.y, 3, 1),
                action,
                selected,
            );
        }
    }
}

fn activity(app: &App) -> String {
    if !matches!(app.connection, ConnectionState::Connected { .. }) {
        return String::new();
    }
    let Some(snapshot) = &app.chat.snapshot else {
        return String::new();
    };
    let key = if !snapshot.interactions.pending().is_empty() {
        Some("session-waiting")
    } else if let Some(turn) = &snapshot.root_turn {
        match turn.state {
            TurnState::Admitted(_) | TurnState::Created(_) | TurnState::Running(_) => None,
            TurnState::WaitingForUser(_) => Some("session-waiting"),
            // Terminal outcomes belong to their transcript turn, not the next draft.
            TurnState::Failed { .. }
            | TurnState::Cancelled { .. }
            | TurnState::Completed { .. } => None,
        }
    } else {
        match snapshot.session.status {
            SessionStatus::Running => None,
            SessionStatus::WaitingForUser => Some("session-waiting"),
            SessionStatus::Blocked => Some("session-blocked"),
            SessionStatus::Aborted => Some("session-aborted"),
            SessionStatus::Active => None,
        }
    };
    key.map_or_else(String::new, |key| app.i18n.text(key))
}

fn sandbox_key(mode: maka_sandbox::Mode) -> &'static str {
    match mode {
        maka_sandbox::Mode::ReadOnly => "chat-sandbox-read",
        maka_sandbox::Mode::WorkspaceWrite => "chat-sandbox-workspace",
        maka_sandbox::Mode::DangerFullAccess => "chat-sandbox-none",
    }
}

fn metadata(app: &App, width: u16) -> (Line<'static>, u16) {
    let Detail::Ready(item) = &app.sessions.detail else {
        return (Line::default(), 0);
    };
    let sandbox = app.i18n.text(sandbox_key(item.sandbox_mode));
    let context = app
        .chat
        .context
        .current_label(
            &item.model,
            item.llm_connection_id.as_deref(),
            app.chrome.ascii,
        )
        .filter(|_| {
            app.chat.error.is_none() && matches!(app.connection, ConnectionState::Connected { .. })
        })
        .filter(|text| usize::from(width) >= sandbox.width() + text.width() + 15);
    let model = safe(&item.model);
    let available = usize::from(width)
        .saturating_sub(sandbox.width() + 8 + context.as_ref().map_or(0, |text| text.width() + 3));
    let thinking = item.thinking_level.map(|level| {
        app.i18n
            .text(crate::pages::manage::models::thinking_key(Some(level)))
    });
    let model = if available >= 6 {
        match thinking.filter(|level| available >= level.width() + 9) {
            Some(level) => format!("{} · {level}", fit(&model, available - level.width() - 3)),
            None => fit(&model, available),
        }
    } else {
        String::new()
    };
    let warning = item.sandbox_mode == maka_sandbox::Mode::DangerFullAccess;
    let mut spans = vec![Span::raw(" ")];
    if !model.is_empty() {
        spans.push(Span::raw(format!("{model} · ")));
    }
    if let Some(context) = context {
        spans.push(Span::raw(format!("{context} · ")));
    }
    spans.push(Span::styled(
        sandbox,
        Style::default().fg(if warning {
            app.theme.colors().warning
        } else {
            app.theme.colors().subtle
        }),
    ));
    // Only label padding; the right offset belongs to its rectangle so the border continues.
    spans.push(Span::raw(" "));
    (
        Line::from(spans).style(Style::default().fg(app.theme.colors().subtle)),
        model.width() as u16,
    )
}

pub(super) fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut cells = 0;
    for grapheme in text.graphemes(true) {
        cells += grapheme.width();
        if cells + 1 > width {
            break;
        }
        result.push_str(grapheme);
    }
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Locale, LocalePreference, i18n::I18n};
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend, layout::Position};

    #[test]
    fn stop_is_icon_only_run_scoped_and_preserves_the_draft_until_observed_terminal() {
        for locale in Locale::ALL {
            let mut app = App::new(
                "/fixture".into(),
                I18n::new(LocalePreference::Explicit(locale), locale),
            );
            app.apply(Action::Visit(Route::Session("chat".into())));
            app.connection = ConnectionState::Connected {
                root_id: "root".into(),
                epoch: "epoch".into(),
            };
            app.chat.select(&Route::Session("chat".into()));
            let snapshot = maka_protocol::subscription::decode_session_observation_snapshot(&serde_json::json!({
                "schemaVersion":5,"session":{"sessionId":"chat","metadataRevision":1,"status":"running","createdAt":0,"isArchived":false},
                "projectionRevision":1,"rootTurn":{"sessionId":"chat","turnId":"turn","runId":"run","status":"running"},
                "goal":null,"queue":{"hostEpoch":"epoch","queueRevision":0,"steering":[],"followup":[]},"interactions":{"pending":[]}
            })).unwrap();
            app.chat.snapshot = Some(snapshot.clone());
            app.chrome.motion = false;
            assert!(
                activity(&app).is_empty(),
                "the stop icon already communicates an active run"
            );
            app.input(Event::Paste("next draft 中文🦀".into()));
            let target = app.stop_target().unwrap();
            let action = Action::StopTurn(target.clone());
            assert_eq!(app.send_action(), action);
            for (width, height) in [(30, 10), (80, 24), (120, 40)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| super::super::draw(frame, &mut app))
                    .unwrap();
                let hits: Vec<_> = app.hits.iter().filter(|hit| hit.action == action).collect();
                assert_eq!(hits.len(), 1);
                let area = hits[0].area;
                let button: String = (area.x..area.right())
                    .map(|x| terminal.backend().buffer()[(x, area.y)].symbol())
                    .collect();
                assert_eq!(button, " ■ ");
                assert_eq!(
                    terminal.backend().buffer()[(area.x, area.y - 1)].fg,
                    tone::border(app.theme.colors(), true, Some(0.45))
                );
                assert!(!app.drafts["chat"].contains(Position::new(area.x, area.y)));
                assert_eq!(
                    app.input(Event::Mouse(MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column: area.x,
                        row: area.y,
                        modifiers: KeyModifiers::NONE,
                    }))
                    .1,
                    Some(action.clone())
                );
            }
            assert!(app.chat.start_stop(&target));
            assert!(!app.enabled(&action));
            assert!(!app.chat.start_stop(&target));
            app.chat
                .stopped(target.clone(), Ok(snapshot.root_turn.clone().unwrap()));
            assert!(
                !app.enabled(&action),
                "a running acknowledgement is not completion"
            );
            assert_eq!(app.drafts["chat"].text(), "next draft 中文🦀");
            app.chrome.motion = true;
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal
                .draw(|frame| super::super::draw(frame, &mut app))
                .unwrap();
            assert!(
                app.chrome
                    .animation
                    .wait(std::time::Instant::now())
                    .is_some()
            );
            app.input(Event::FocusLost);
            terminal
                .draw(|frame| super::super::draw(frame, &mut app))
                .unwrap();
            assert!(
                app.chrome
                    .animation
                    .wait(std::time::Instant::now())
                    .is_none()
            );
            app.input(Event::FocusGained);
            app.palette = Some(0);
            terminal
                .draw(|frame| super::super::draw(frame, &mut app))
                .unwrap();
            assert!(
                app.chrome
                    .animation
                    .wait(std::time::Instant::now())
                    .is_none()
            );
            app.palette = None;
            app.chat
                .snapshot
                .as_mut()
                .unwrap()
                .root_turn
                .as_mut()
                .unwrap()
                .state = TurnState::WaitingForUser(Default::default());
            terminal
                .draw(|frame| super::super::draw(frame, &mut app))
                .unwrap();
            assert!(
                app.chrome
                    .animation
                    .wait(std::time::Instant::now())
                    .is_none(),
                "waiting for the user is not working"
            );
            app.chat
                .snapshot
                .as_mut()
                .unwrap()
                .root_turn
                .as_mut()
                .unwrap()
                .state = TurnState::Cancelled {
                terminal_event_id: "end".into(),
                abort_source: "user".into(),
            };
            assert_eq!(app.send_action(), Action::SendMessage);
            terminal
                .draw(|frame| super::super::draw(frame, &mut app))
                .unwrap();
            let send = app
                .hits
                .iter()
                .find(|hit| hit.action == Action::SendMessage)
                .unwrap();
            assert_ne!(
                terminal.backend().buffer()[(send.area.x, send.area.y - 1)].fg,
                tone::border(app.theme.colors(), true, Some(0.45))
            );
            app.chat.snapshot = Some(snapshot.clone());
            app.chat
                .snapshot
                .as_mut()
                .unwrap()
                .root_turn
                .as_mut()
                .unwrap()
                .run_id = "next-run".into();
            assert!(!app.enabled(&action), "old hit cannot stop the next run");
            let next = app.stop_target().unwrap();
            assert!(app.chat.start_stop(&next));
            app.chat.stopped(
                target.clone(),
                Err(maka_client::RequestFailure::Unknown(
                    maka_client::ClientError::Protocol("late".into()),
                )),
            );
            assert!(app.chat.stop.pending(&next));
            app.chat.select(&Route::Workspace);
            app.chat.select(&Route::Session("chat".into()));
            app.chat.snapshot = Some(snapshot);
            assert!(
                !app.enabled(&action),
                "reopening invalidates rendered actions"
            );
            assert_eq!(app.drafts["chat"].text(), "next draft 中文🦀");
        }
    }

    #[test]
    fn composer_owns_send_geometry_and_keeps_risk_visible_across_sizes_and_locales() {
        for locale in Locale::ALL {
            let mut app = App::new(
                "/fixture".into(),
                I18n::new(LocalePreference::Explicit(locale), locale),
            );
            app.apply(Action::Visit(Route::Session("chat".into())));
            app.connection = ConnectionState::Connected {
                root_id: "r".into(),
                epoch: "e".into(),
            };
            app.sessions.detail = Detail::Ready(Box::new(maka_protocol::session::decode_session_catalog_projection(&serde_json::json!({
                "id":"chat","revision":1,"workspace":{"target":{"kind":"host_path","path":"/work"},"hostCwd":"/work"},
                "createdAt":0,"activityAt":1,"name":"Design session","isFlagged":false,"isArchived":false,
                "labels":[],"labelsTruncated":false,"hasUnread":false,"status":"active","backend":"ai-sdk",
                "llmConnectionId":"connection","llmConnectionSlug":"default","connectionLocked":false,"model":"very-long-model-中文-model-name",
                "sandboxMode":"danger-full-access","approvalPolicy":{"kind":"on-request"},"collaborationMode":"agent","orchestrationMode":"default","thinkingLevel":"high"
            })).unwrap()));
            app.chat.context.complete(Ok(maka_protocol::context::decode_context_diagnostics_result(&serde_json::json!({
                "status":"available","providerId":"openai","modelId":"very-long-model-中文-model-name",
                "completedAt":1,"inputTokens":9500,"contextWindow":128000,
                "current":{"connectionId":"connection","tokens":10500,"approximate":true}
            })).unwrap()));
            app.input(Event::Paste("first\n中文🦀 second".into()));
            for (width, height) in [(30, 10), (80, 24), (120, 40)] {
                for ascii in [false, true] {
                    app.chrome.ascii = ascii;
                    app.focus = Focus::Composer;
                    app.hover = None;
                    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                    terminal
                        .draw(|frame| super::super::draw(frame, &mut app))
                        .unwrap();
                    let mut text = String::new();
                    for row in terminal
                        .backend()
                        .buffer()
                        .content
                        .chunks(usize::from(width))
                    {
                        let mut x = 0;
                        while x < row.len() {
                            let symbol = row[x].symbol();
                            text.push_str(symbol);
                            x += symbol.width().max(1);
                        }
                        text.push('\n');
                    }
                    assert!(text.contains(&app.i18n.text("chat-sandbox-none")));
                    if width == 120 {
                        assert!(text.contains(if ascii {
                            "~10.5k / 128.0k"
                        } else {
                            "≈10.5k / 128.0k"
                        }));
                    }
                    if width == 30 {
                        assert!(
                            !text.contains("128.0k"),
                            "context cannot crowd out input and sandbox"
                        );
                    }
                    assert!(
                        terminal
                            .backend()
                            .buffer()
                            .content
                            .iter()
                            .rev()
                            .take(usize::from(width))
                            .all(|cell| cell.symbol() == " "),
                        "idle composer has no footer tutorial"
                    );
                    if width >= 80 {
                        let first = &terminal.backend().buffer().content[..usize::from(width)];
                        assert_eq!(
                            first.iter().position(|cell| cell.symbol() == "D"),
                            Some((usize::from(width) - "Design session".len()) / 2),
                            "session title is centered independently of the action cluster"
                        );
                    }
                    assert!(!text.contains("Ctrl+Z") && !text.contains("Draft"));
                    let model_hit = app.hits.iter().find(|hit| {
                        matches!(
                            &hit.action,
                            Action::Manage(crate::pages::manage::Command::Open(
                                _,
                                crate::pages::manage::Kind::Model
                            ))
                        )
                    });
                    if width >= 80 {
                        let hit = model_hit.expect("model label opens model selection");
                        assert!(text.contains(&format!(" · {}", app.i18n.text("thinking-high"))));
                        assert!(
                            super::super::action_label(&app, &hit.action)
                                .contains(&app.i18n.text("thinking-high"))
                        );
                        let buffer = terminal.backend().buffer();
                        let corner = (0..width)
                            .rev()
                            .find(|x| {
                                buffer[(*x, hit.area.y)].symbol()
                                    == if ascii { "┘" } else { "╯" }
                            })
                            .expect("composer keeps its bottom-right corner");
                        for x in corner - 3..corner {
                            assert_eq!(
                                buffer[(x, hit.area.y)].symbol(),
                                "─",
                                "metadata offset must not erase the border"
                            );
                        }
                        assert_eq!(buffer[(hit.area.x, hit.area.y)].symbol(), "v");
                        assert_eq!(
                            buffer[(hit.area.right(), hit.area.y)].symbol(),
                            " ",
                            "model hit excludes the separator, context count and sandbox"
                        );
                        assert_eq!(hit.area.height, 1);
                    }
                    let sends: Vec<_> = app
                        .hits
                        .iter()
                        .filter(|hit| hit.action == Action::SendMessage)
                        .collect();
                    assert_eq!(sends.len(), 1, "one send button, inside composer");
                    let send = sends[0].area;
                    assert!(send.y >= height.saturating_sub(6));
                    assert!(!app.drafts["chat"].contains(Position::new(send.x, send.y)));
                    assert_eq!(
                        app.input(Event::Mouse(MouseEvent {
                            kind: MouseEventKind::Down(MouseButton::Left),
                            column: send.x,
                            row: send.y,
                            modifiers: KeyModifiers::NONE
                        }))
                        .1,
                        Some(Action::SendMessage)
                    );
                    assert_eq!(app.drafts["chat"].text(), "first\n中文🦀 second");
                    terminal
                        .draw(|frame| super::super::draw(frame, &mut app))
                        .unwrap();
                    let footer =
                        &terminal.backend().buffer().content[usize::from((height - 1) * width)..];
                    let hint = app.i18n.text("chat-send");
                    if hint.width() <= usize::from(width) {
                        let left = footer.iter().position(|cell| cell.symbol() != " ").unwrap();
                        let pane_left =
                            usize::from(app.chrome.sidebar_width(width, std::time::Instant::now()))
                                + 1;
                        let right = usize::from(width) - 1 - hint.width() - left;
                        assert!(
                            left >= pane_left && (left - pane_left).abs_diff(right) <= 1,
                            "necessary footer hints center in the active pane, excluding the sidebar"
                        );
                    }
                }
            }
        }
    }
}
