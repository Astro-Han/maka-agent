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

use super::{Action, App, Command, Receipt};
use crate::view::{button, safe};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::Style,
    widgets::{Block, Paragraph, Wrap},
};
impl App {
    pub fn recap_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                KeyCode::Esc => Some(Command::Close),
                KeyCode::Tab | KeyCode::Right => {
                    self.recap.focus = (self.recap.focus + 1) % self.recap_buttons().len();
                    None
                }
                KeyCode::BackTab | KeyCode::Left => {
                    self.recap.focus = (self.recap.focus + self.recap_buttons().len() - 1)
                        % self.recap_buttons().len();
                    None
                }
                KeyCode::Down => {
                    self.recap.scroll = self.recap.scroll.saturating_add(1);
                    None
                }
                KeyCode::Up => {
                    self.recap.scroll = self.recap.scroll.saturating_sub(1);
                    None
                }
                KeyCode::PageDown => {
                    self.recap.scroll = self.recap.scroll.saturating_add(10);
                    None
                }
                KeyCode::PageUp => {
                    self.recap.scroll = self.recap.scroll.saturating_sub(10);
                    None
                }
                KeyCode::Home => {
                    self.recap.scroll = 0;
                    None
                }
                KeyCode::Enter => Some(self.recap_buttons()[self.recap.focus].clone()),
                _ => None,
            },
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                self.hits.iter().find_map(|hit| match &hit.action {
                    Action::Recap(command)
                        if hit.area.contains((mouse.column, mouse.row).into()) =>
                    {
                        Some(command.clone())
                    }
                    _ => None,
                })
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::ScrollDown => {
                self.recap.scroll = self.recap.scroll.saturating_add(3);
                None
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::ScrollUp => {
                self.recap.scroll = self.recap.scroll.saturating_sub(3);
                None
            }
            _ => None,
        };
        (true, command.and_then(|c| self.apply(Action::Recap(c))))
    }
    fn recap_buttons(&self) -> Vec<Command> {
        let mut buttons = vec![
            Command::Close,
            Command::Read,
            if self.recap.saved.is_some() {
                Command::Retry
            } else {
                Command::Generate
            },
        ];
        if self.recap.saved.is_some() {
            buttons.push(if self.recap.discarding {
                Command::ConfirmForget
            } else {
                Command::Forget
            });
        }
        buttons
    }
}
pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let width = area.width.saturating_sub(2).min(78);
    let height = area.height.saturating_sub(2).min(24);
    if width < 48 || height < 12 {
        app.recap.rendered = false;
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    app.recap.rendered = true;
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(app.i18n.text("recap-title"))
        .title_alignment(Alignment::Center)
        .style(base);
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    app.recap.focus = app.recap.focus.min(app.recap_buttons().len() - 1);
    let state = &app.recap;
    let mut text = state
        .target
        .as_ref()
        .map(|t| safe(&t.session))
        .unwrap_or_default();
    text.push_str("\n\n");
    text.push_str(&app.i18n.text("recap-note"));
    text.push_str("\n\n");
    if state.discarding {
        text.push_str(&app.i18n.text("recap-forget-note"));
        text.push_str("\n\n");
    }
    if state.pending.is_some() {
        text.push_str(&app.i18n.text("recap-working"));
        text.push_str("\n\n");
    }
    if state.saved.is_some() {
        text.push_str(&app.i18n.text("recap-unresolved"));
        text.push_str("\n\n");
    }
    if let Some(error) = &state.error {
        text.push_str(&safe(error));
        text.push_str("\n\n");
    }
    match &state.receipt {
        Some(Receipt::Ready {
            text: summary,
            model_id,
            ..
        }) => {
            text.push_str(&safe(summary));
            text.push_str("\n\n");
            text.push_str(&safe(model_id));
        }
        Some(Receipt::Pending { .. }) => text.push_str(&app.i18n.text("recap-pending")),
        Some(Receipt::Failed { reason, .. }) => {
            text.push_str(&app.i18n.text("recap-failed"));
            text.push('\n');
            text.push_str(&safe(reason));
        }
        None if state.loaded => text.push_str(&app.i18n.text("recap-empty")),
        None => {}
    }
    let lines = crate::pages::manage::view::note_lines(&text, inner.width);
    let available = inner.height.saturating_sub(3);
    app.recap.scroll = app
        .recap
        .scroll
        .min((lines.len() as u16).saturating_sub(available));
    frame.render_widget(
        Paragraph::new(lines).scroll((app.recap.scroll, 0)),
        Rect::new(inner.x, inner.y, inner.width, available),
    );
    let buttons = app.recap_buttons();
    let size = inner.width / buttons.len() as u16;
    for (i, command) in buttons.into_iter().enumerate() {
        let label = app.i18n.text(command.label());
        button(
            frame,
            app,
            Rect::new(
                inner.x + i as u16 * size,
                inner.bottom() - 1,
                size.saturating_sub(1),
                1,
            ),
            &label,
            Action::Recap(command),
            app.recap.focus == i,
        );
    }
}
