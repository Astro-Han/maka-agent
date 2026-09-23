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

use super::super::{Command as Models, Manage};
use super::{Command, Field};
use crate::{
    app::{Action, App, Hit},
    view::{button, safe, tone},
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    widgets::{Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use unicode_width::UnicodeWidthStr;

fn action(command: Command) -> Action {
    Action::Manage(Manage::EnabledModels(Models::Profile(command)))
}
pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let busy = app.management.pending.is_some();
    let dialog = app.management.dialog.as_mut().unwrap();
    let draft = dialog
        .enabled_models
        .as_mut()
        .unwrap()
        .profile
        .as_mut()
        .unwrap();
    if area.width < 42 || area.height < 17 {
        dialog.visible = false;
        draft.invalidate_geometry();
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let width = area.width.saturating_sub(2).min(84);
    let height = area.height.saturating_sub(2).min(28);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .title(app.i18n.text("connection-model-overrides"))
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    app.modal_area = Some(popup);
    dialog.visible = true;
    let enabled = !busy && !dialog.blocked;
    let title = Rect::new(inner.x, inner.y, inner.width, 1);
    frame.render_widget(
        Paragraph::new(safe(&draft.id))
            .style(Style::default().fg(tone::secondary(app.theme.colors()))),
        title,
    );
    let list = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(8),
    );
    let visible = list.height as usize;
    let selected = draft.focus.min(draft.fields.len() - 1);
    draft.offset = draft.offset.min(draft.fields.len().saturating_sub(visible));
    if selected < draft.offset {
        draft.offset = selected;
    } else if selected >= draft.offset + visible {
        draft.offset = (selected + 1).saturating_sub(visible);
    }
    let offset = draft.offset;
    for text in &mut draft.texts {
        let index = draft
            .fields
            .iter()
            .position(|field| *field == text.field)
            .unwrap();
        if !(offset..offset + visible).contains(&index) {
            text.editor.invalidate_geometry();
        }
    }
    for index in offset..(offset + visible).min(draft.fields.len()) {
        let field = draft.fields[index];
        let y = list.y + (index - offset) as u16;
        let label_width = 20.min(list.width.saturating_sub(12));
        let row = Rect::new(list.x, y, list.width.saturating_sub(1), 1);
        let value_area = Rect::new(
            list.x + label_width,
            y,
            row.width.saturating_sub(label_width),
            1,
        );
        let chosen = draft.focus == index;
        if chosen {
            frame.render_widget(
                Paragraph::new("").style(tone::selection(app.theme.colors())),
                row,
            );
        }
        let label = if let Field::Level(level) = field {
            app.i18n.format(
                "model-profile-level",
                &[(
                    "level",
                    &app.i18n
                        .text(crate::pages::manage::models::thinking_key(Some(level))),
                )],
            )
        } else if let Field::Modality(direction, kind) = field {
            app.i18n.text(&format!("model-profile-{direction}-{kind}"))
        } else {
            app.i18n.text(field.label())
        };
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(if chosen {
                tone::accent(app.theme.colors())
            } else {
                tone::secondary(app.theme.colors())
            })),
            Rect::new(list.x, y, label_width.saturating_sub(1), 1),
        );
        if enabled {
            app.hits.push(Hit {
                area: row,
                action: action(Command::Field(index)),
            });
        }
        if let Some(text) = draft.texts.iter_mut().find(|text| text.field == field) {
            text.editor
                .draw(frame, value_area, enabled && chosen, app.theme.colors());
            if text.editor.text().is_empty() {
                frame.render_widget(
                    Paragraph::new(app.i18n.text("thinking-default"))
                        .style(Style::default().fg(app.theme.colors().subtle)),
                    value_area,
                );
            }
        } else {
            let display = match field {
                Field::Level(level) => {
                    if draft.values["thinkingLevels"]
                        .as_array()
                        .is_some_and(|levels| levels.contains(&serde_json::json!(level)))
                    {
                        app.chrome.symbol("[✓]", "[x]").to_owned()
                    } else {
                        "[ ]".into()
                    }
                }
                Field::Boolean(_) | Field::Capability(_) => {
                    app.i18n.text(match draft.field_value(field).as_bool() {
                        Some(true) => "model-profile-on",
                        Some(false) => "model-profile-off",
                        None => "thinking-default",
                    })
                }
                Field::Protocol => draft.values["apiProtocol"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| app.i18n.text("thinking-default")),
                Field::Advanced => if index + 1 < draft.fields.len() {
                    app.chrome.symbol("⌄", "v")
                } else {
                    app.chrome.symbol("›", ">")
                }
                .into(),
                Field::Modalities => app.i18n.text(if draft.values["modalities"].is_object() {
                    "model-profile-custom"
                } else {
                    "thinking-default"
                }),
                Field::Modality(direction, kind) => {
                    if !draft.values["modalities"].is_object() {
                        app.i18n.text("thinking-default")
                    } else if draft.values["modalities"][direction]
                        .as_array()
                        .unwrap()
                        .contains(&serde_json::json!(kind))
                    {
                        app.chrome.symbol("[✓]", "[x]").into()
                    } else {
                        "[ ]".into()
                    }
                }
                Field::ServiceTier => {
                    if draft.values["serviceTier"].is_null() {
                        app.i18n.text("thinking-default")
                    } else {
                        "fast".into()
                    }
                }
                _ => unreachable!(),
            };
            frame.render_widget(
                Paragraph::new(display).style(Style::default().fg(
                    if enabled && draft.accepts(&Command::Adjust(index, true)) {
                        tone::accent(app.theme.colors())
                    } else {
                        app.theme.colors().subtle
                    },
                )),
                value_area,
            );
            if enabled && draft.accepts(&Command::Adjust(index, true)) {
                app.hits.push(Hit {
                    area: value_area,
                    action: action(Command::Adjust(index, true)),
                });
            }
        }
    }
    if draft.fields.len() > visible {
        let mut scroll =
            ScrollbarState::new(draft.fields.len().saturating_sub(visible) + 1).position(offset);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_style(Style::default().fg(app.theme.colors().subtle)),
            list,
            &mut scroll,
        );
    }
    let focus = draft.focus;
    let count = draft.fields.len();
    let error = dialog
        .error
        .or_else(|| draft.value().err())
        .or_else(|| draft.texts.iter().find_map(|text| text.editor.error));
    let note = app.i18n.text(if busy {
        "session-saving"
    } else {
        error.unwrap_or(
            if draft
                .fields
                .get(focus)
                .is_some_and(|field| matches!(field, Field::Modalities | Field::Modality(_, _)))
            {
                "model-profile-modalities-note"
            } else {
                "model-profile-note"
            },
        )
    });
    let lines = crate::pages::manage::view::note_lines(&note, inner.width);
    dialog.visible = lines.len() <= 4;
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(if error.is_some() {
            app.theme.colors().warning
        } else {
            tone::secondary(app.theme.colors())
        })),
        Rect::new(inner.x, inner.bottom() - 5, inner.width, 4),
    );
    let back = app.i18n.text("model-profile-back");
    let save = app.i18n.text("session-save");
    let cancel = app.i18n.text("session-cancel");
    let bw = back.width() as u16 + 2;
    let sw = save.width() as u16 + 2;
    let cw = cancel.width() as u16 + 2;
    let y = inner.bottom() - 1;
    button(
        frame,
        app,
        Rect::new(inner.x, y, bw, 1),
        &back,
        action(Command::Back),
        focus == count,
    );
    button(
        frame,
        app,
        Rect::new(inner.right() - sw - cw - 1, y, cw, 1),
        &cancel,
        Action::Manage(Manage::Close),
        focus == count + 1,
    );
    button(
        frame,
        app,
        Rect::new(inner.right() - sw, y, sw, 1),
        &save,
        Action::Manage(Manage::Save),
        focus == count + 2,
    );
}

pub fn input(app: &mut App, event: Event) -> (bool, Option<Action>) {
    let dialog = app.management.dialog.as_mut().unwrap();
    let draft = dialog
        .enabled_models
        .as_mut()
        .unwrap()
        .profile
        .as_mut()
        .unwrap();
    let enabled = dialog.visible && !dialog.blocked && app.management.pending.is_none();
    let total = draft.fields.len() + 3;
    let command = match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
            KeyCode::Esc => Some(Action::Manage(Manage::Close)),
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return (true, Some(Action::Quit));
            }
            _ if !enabled => None,
            KeyCode::Tab | KeyCode::BackTab => {
                draft.focus = (draft.focus
                    + if key.code == KeyCode::Tab {
                        1
                    } else {
                        total - 1
                    })
                    % total;
                return (true, None);
            }
            KeyCode::Down | KeyCode::Up | KeyCode::PageDown | KeyCode::PageUp => {
                let delta = match key.code {
                    KeyCode::Up => -1,
                    KeyCode::PageUp => -6,
                    KeyCode::PageDown => 6,
                    _ => 1,
                };
                draft.focus = draft.focus.saturating_add_signed(delta).min(total - 1);
                return (true, None);
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Action::Manage(Manage::Save))
            }
            KeyCode::Enter if draft.focus == total - 3 => Some(action(Command::Back)),
            KeyCode::Enter if draft.focus == total - 2 => Some(Action::Manage(Manage::Close)),
            KeyCode::Enter if draft.focus == total - 1 => Some(Action::Manage(Manage::Save)),
            _ => {
                let Some(&field) = draft.fields.get(draft.focus) else {
                    return (false, None);
                };
                if let Some(text) = draft.texts.iter_mut().find(|text| text.field == field) {
                    if key.code == KeyCode::Enter {
                        draft.focus = (draft.focus + 1) % total;
                        return (true, None);
                    }
                    return (text.editor.key(key), None);
                }
                match key.code {
                    KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right => {
                        Some(action(Command::Adjust(draft.focus, true)))
                    }
                    KeyCode::Left => Some(action(Command::Adjust(draft.focus, false))),
                    KeyCode::Backspace | KeyCode::Delete => {
                        Some(action(Command::Default(draft.focus)))
                    }
                    _ => None,
                }
            }
        },
        Event::Paste(text) if enabled && !text.chars().any(char::is_control) => {
            let Some(field) = draft.fields.get(draft.focus) else {
                return (false, None);
            };
            let Some(editor) = draft.texts.iter_mut().find(|text| text.field == *field) else {
                return (false, None);
            };
            return (editor.editor.insert(&text), None);
        }
        Event::Mouse(mouse) if dialog.visible => {
            if enabled
                && matches!(
                    mouse.kind,
                    MouseEventKind::Down(MouseButton::Left)
                        | MouseEventKind::Drag(MouseButton::Left)
                        | MouseEventKind::Up(MouseButton::Left)
                )
            {
                for text in &mut draft.texts {
                    if text.editor.contains((mouse.column, mouse.row).into())
                        || text.editor.dragging()
                    {
                        draft.focus = draft
                            .fields
                            .iter()
                            .position(|field| *field == text.field)
                            .unwrap();
                        return (text.editor.mouse(mouse), None);
                    }
                }
            }
            let hit = app
                .hits
                .iter()
                .rev()
                .find(|hit| hit.area.contains((mouse.column, mouse.row).into()))
                .map(|hit| hit.action.clone());
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => hit,
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                    if enabled
                        && matches!(
                            hit,
                            Some(Action::Manage(Manage::EnabledModels(Models::Profile(_))))
                        ) =>
                {
                    draft.focus = draft
                        .focus
                        .saturating_add_signed(if mouse.kind == MouseEventKind::ScrollDown {
                            1
                        } else {
                            -1
                        })
                        .min(draft.fields.len() - 1);
                    return (true, None);
                }
                _ => None,
            }
        }
        _ => None,
    };
    (
        command.is_some(),
        command.and_then(|command| app.apply(command)),
    )
}
