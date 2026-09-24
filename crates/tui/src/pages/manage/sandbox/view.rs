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

use super::{Command, Manage, approval_label, bypass};
use crate::{
    app::{Action, App, Hit},
    view::{button, tone},
};
use maka_protocol::session::SandboxMode;
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    widgets::{Block, Paragraph, Wrap},
};

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let dialog = app.management.dialog.as_ref().unwrap();
    let state = dialog.sandbox.as_ref().unwrap();
    let controls = state.controls();
    let width = area.width.saturating_sub(2).min(64);
    let full_bypass = bypass(state.mode, state.approval);
    let danger = state.mode == SandboxMode::DangerFullAccess;
    let note = app.i18n.text(if !state.loaded() {
        "sandbox-default-loading"
    } else if state.defaults.is_some() && !danger {
        "sandbox-default-note"
    } else if full_bypass {
        "session-sandbox-bypass-warning"
    } else if danger {
        "session-sandbox-warning"
    } else if state.approvals {
        "session-approval-note"
    } else {
        "session-sandbox-note"
    });
    let mut notes = super::super::view::note_lines(&note, width.saturating_sub(4));
    if state.defaults.is_some() && danger {
        notes.extend(super::super::view::note_lines(
            &app.i18n.text("sandbox-default-note"),
            width.saturating_sub(4),
        ));
    }
    if let Some(error) = dialog.error {
        notes.push(Default::default());
        notes.extend(super::super::view::note_lines(
            &app.i18n.text(error),
            width.saturating_sub(4),
        ));
    }
    // Radio rows have a blank line; granular checkboxes form a compact group.
    let offsets: Vec<u16> = (0..controls.len())
        .map(|i| {
            if i < 4 {
                1 + i as u16 * 2
            } else if state.approvals {
                9 + (i - 4) as u16
            } else {
                9
            }
        })
        .collect();
    let note_y = offsets.last().copied().unwrap_or(0) + 2;
    let height = note_y + notes.len() as u16 + 5;
    if width < 34 || area.height < height + 2 {
        app.management.dialog.as_mut().unwrap().visible = false;
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .border_type(if app.chrome.ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        })
        .title_top(
            ratatui::text::Line::raw(app.i18n.text(if state.defaults.is_some() {
                "sandbox-default-title"
            } else if state.approvals {
                "session-approval-title"
            } else {
                "session-sandbox-change"
            }))
            .centered(),
        )
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().accent));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    app.modal_area = Some(popup);
    let dialog = app.management.dialog.as_mut().unwrap();
    dialog.visible = true;
    let state = dialog.sandbox.as_ref().unwrap();
    let focus = dialog.focus;
    let enabled = !dialog.blocked && app.management.pending.is_none();
    for (index, command) in controls.iter().copied().enumerate() {
        let selected = state.selected(command);
        let label = match command {
            Command::Approvals(true) => format!(
                "{} · {} {}",
                app.i18n.text(command.label()),
                app.i18n.text(approval_label(state.approval)),
                app.chrome.symbol("›", ">")
            ),
            Command::Approvals(false) => format!(
                "{} {}",
                app.chrome.symbol("‹", "<"),
                app.i18n.text(command.label())
            ),
            _ => {
                let marker = if matches!(command, Command::Category(_)) {
                    if selected {
                        app.chrome.symbol("☑", "[x]")
                    } else {
                        app.chrome.symbol("☐", "[ ]")
                    }
                } else if selected {
                    app.chrome.symbol("●", "[x]")
                } else {
                    app.chrome.symbol("○", "[ ]")
                };
                format!("{marker} {}", app.i18n.text(command.label()))
            }
        };
        let row = Rect::new(inner.x, inner.y + offsets[index], inner.width, 1);
        frame.render_widget(
            Paragraph::new(label).style(if !enabled {
                Style::default().fg(app.theme.colors().subtle)
            } else if focus == index {
                tone::selection(app.theme.colors()).fg(app.theme.colors().accent)
            } else if matches!(
                command,
                Command::Bypass | Command::Mode(SandboxMode::DangerFullAccess)
            ) {
                Style::default().fg(app.theme.colors().warning)
            } else {
                Style::default()
            }),
            row,
        );
        if enabled {
            app.hits.push(Hit {
                area: row,
                action: Action::Manage(Manage::Sandbox(command)),
            });
        }
    }
    frame.render_widget(
        Paragraph::new(notes).style(Style::default().fg(if danger || dialog.error.is_some() {
            app.theme.colors().warning
        } else {
            app.theme.colors().muted
        })),
        Rect::new(
            inner.x,
            inner.y + note_y,
            inner.width,
            inner.height.saturating_sub(note_y + 3),
        ),
    );
    let save = if full_bypass {
        "session-sandbox-enable-bypass"
    } else if danger && state.mode_patch().is_some() {
        "session-sandbox-disable"
    } else {
        "session-save"
    };
    for (index, (command, key)) in [(Manage::Close, "session-cancel"), (Manage::Save, save)]
        .into_iter()
        .enumerate()
    {
        button(
            frame,
            app,
            Rect::new(
                inner.x + inner.width / 2 * index as u16,
                popup.bottom() - 3,
                inner.width / 2,
                1,
            ),
            &app.i18n.text(key),
            Action::Manage(command),
            focus == index + controls.len(),
        );
    }
}
