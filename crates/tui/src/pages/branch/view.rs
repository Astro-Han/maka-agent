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

use super::{Action, App, Command, Phase};
use crate::{
    pages::manage::view::note_lines,
    view::{button, safe},
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::Style,
    widgets::{Block, BorderType, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

impl App {
    pub fn branch_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                KeyCode::Esc => Some(Command::Close),
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                    self.branch.focus = if primary(self.branch.phase)
                        .as_ref()
                        .is_some_and(|command| self.branch_enabled(command))
                    {
                        self.branch.focus ^ 1
                    } else {
                        0
                    };
                    None
                }
                KeyCode::Enter => {
                    if self.branch.focus == 0 {
                        Some(Command::Close)
                    } else {
                        primary(self.branch.phase)
                    }
                }
                _ => None,
            },
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                self.hits.iter().find_map(|hit| match &hit.action {
                    Action::Branch(command)
                        if hit.area.contains((mouse.column, mouse.row).into()) =>
                    {
                        Some(command.clone())
                    }
                    _ => None,
                })
            }
            _ => None,
        };
        (
            true,
            command.and_then(|command| self.apply(Action::Branch(command))),
        )
    }
}
fn primary(phase: Phase) -> Option<Command> {
    match phase {
        Phase::Confirm => Some(Command::Confirm),
        Phase::Unknown => Some(Command::Query),
        Phase::Ready => Some(Command::Visit),
        _ => None,
    }
}
pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let width = area.width.saturating_sub(2).min(62);
    let state = &app.branch;
    let name = state
        .basis
        .as_ref()
        .map(|basis| basis.name.as_str())
        .unwrap_or("");
    let mut heading = safe(name);
    if let Some(basis) = &state.basis {
        heading.push_str("\n\n");
        heading.push_str(&safe(&basis.excerpt));
    }
    let title = note_lines(&heading, width.saturating_sub(4));
    let key = state.error.unwrap_or(match state.phase {
        Phase::Confirm => "branch-note",
        Phase::Saving | Phase::Pending => "branch-wait",
        Phase::Unknown => "branch-unknown",
        Phase::Ready => "branch-ready",
        Phase::Failed => "branch-unavailable",
    });
    let note = note_lines(&app.i18n.text(key), width.saturating_sub(4));
    let height = 7 + title.len() as u16 + note.len() as u16;
    if width < 36 || height > area.height.saturating_sub(2) {
        app.branch.rendered = false;
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let phase = state.phase;
    let focus = state.focus;
    let error = state.error.is_some() || phase == Phase::Unknown;
    app.branch.rendered = true;
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(app.i18n.text("branch-title"))
        .title_alignment(Alignment::Center)
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().accent))
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        });
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    let count = title.len() as u16;
    frame.render_widget(
        Paragraph::new(title),
        Rect::new(inner.x, inner.y + 1, inner.width, count),
    );
    frame.render_widget(
        Paragraph::new(note).style(Style::default().fg(if error {
            app.theme.colors().warning
        } else {
            app.theme.colors().muted
        })),
        Rect::new(
            inner.x,
            inner.y + count + 2,
            inner.width,
            height - count - 5,
        ),
    );
    let cancel = app.i18n.text(if phase == Phase::Confirm {
        "session-cancel"
    } else {
        "session-remove-close"
    });
    let action = primary(phase);
    let label = action
        .as_ref()
        .map(|command| app.i18n.text(command.label()));
    let primary_width = label
        .as_ref()
        .map_or(0, |label| label.width() as u16 + 2)
        .min(inner.width / 2);
    let cancel_width = (cancel.width() as u16 + 2).min(inner.width / 2);
    button(
        frame,
        app,
        Rect::new(
            inner.right() - primary_width - cancel_width - u16::from(primary_width > 0),
            inner.bottom() - 1,
            cancel_width,
            1,
        ),
        &cancel,
        Action::Branch(Command::Close),
        focus == 0,
    );
    if let Some(action) = action {
        button(
            frame,
            app,
            Rect::new(
                inner.right() - primary_width,
                inner.bottom() - 1,
                primary_width,
                1,
            ),
            label.as_ref().unwrap(),
            Action::Branch(action),
            focus == 1,
        );
    }
}
