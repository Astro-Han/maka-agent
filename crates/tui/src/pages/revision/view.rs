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
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::Style,
    widgets::{Block, BorderType, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

fn buttons(app: &App) -> Vec<Command> {
    let state = &app.revision;
    let mut buttons = vec![Command::Close];
    if state.problem.is_some() && !state.confirm_discard {
        buttons.push(Command::Details);
    }
    if state.saved.is_some() && !state.confirm_discard {
        buttons.push(Command::Discard);
    }
    if matches!(state.phase, Phase::UnknownCopy | Phase::UnknownTurn) && !state.confirm_discard {
        buttons.push(Command::Retry);
    }
    if let Some(primary) = state.primary() {
        buttons.push(primary);
    }
    buttons.retain(|command| app.revision_enabled(command));
    buttons
}
impl App {
    pub fn revision_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let mut command = None;
        let controls = buttons(self);
        match &event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                KeyCode::Esc => command = Some(Command::Close),
                KeyCode::Tab | KeyCode::BackTab => {
                    let count = controls.len() + 1;
                    self.revision.focus = if key.code == KeyCode::BackTab {
                        (self.revision.focus + count - 1) % count
                    } else {
                        (self.revision.focus + 1) % count
                    };
                }
                KeyCode::PageUp | KeyCode::PageDown if !self.revision.show_problem => {
                    let index = if key.code == KeyCode::PageUp {
                        self.revision.selected.saturating_sub(1)
                    } else {
                        self.revision.selected + 1
                    };
                    command = Some(Command::Select(index));
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::ALT) => {
                    command = Some(Command::Display)
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    command = Some(Command::Send)
                }
                KeyCode::Enter if self.revision.focus > 0 => {
                    command = controls.get(self.revision.focus - 1).cloned()
                }
                _ => {
                    self.revision_edit(&event);
                }
            },
            Event::Paste(_) => {
                self.revision_edit(&event);
            }
            Event::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    command = self.hits.iter().find_map(|hit| match &hit.action {
                        Action::Revision(command)
                            if hit.area.contains((mouse.column, mouse.row).into()) =>
                        {
                            Some(command.clone())
                        }
                        _ => None,
                    });
                    if self
                        .revision
                        .editor
                        .contains((mouse.column, mouse.row).into())
                        || self
                            .revision
                            .problem
                            .as_ref()
                            .is_some_and(|e| e.contains((mouse.column, mouse.row).into()))
                    {
                        self.revision.focus = 0;
                    }
                }
                if command.is_none() {
                    self.revision_edit(&event);
                }
            }
            _ => {}
        }
        (
            true,
            command.and_then(|command| self.apply(Action::Revision(command))),
        )
    }
    fn revision_edit(&mut self, event: &Event) {
        let state = &mut self.revision;
        if state.rendered && state.show_problem && state.focus == 0 && !state.confirm_discard {
            if let Some(problem) = &mut state.problem {
                match event {
                    Event::Key(key)
                        if matches!(
                            key.code,
                            KeyCode::Left
                                | KeyCode::Right
                                | KeyCode::Up
                                | KeyCode::Down
                                | KeyCode::Home
                                | KeyCode::End
                                | KeyCode::PageUp
                                | KeyCode::PageDown
                        ) =>
                    {
                        problem.key(*key);
                    }
                    Event::Mouse(mouse) => {
                        problem.mouse(*mouse);
                    }
                    _ => {}
                }
            }
            return;
        }
        if !state.rendered
            || state.phase != Phase::Editing
            || state.focus != 0
            || state.confirm_discard
        {
            return;
        }
        let before = state.editor.save();
        match event {
            Event::Key(key) => {
                state.editor.key(*key);
            }
            Event::Paste(text) => {
                state.editor.insert(text);
            }
            Event::Mouse(mouse) => {
                state.editor.mouse(*mouse);
            }
            _ => return,
        }
        if before.text == state.editor.text() {
            return;
        }
        let input = &mut state.saved.as_mut().unwrap().inputs[state.selected];
        match input.replace(state.editor.text().into(), state.display) {
            Ok(()) => state.error = None,
            Err(error) => {
                // Reverse this one edit, preserving the editor's earlier history.
                let redo = matches!(event, Event::Key(key) if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('z') && !key.modifiers.contains(KeyModifiers::SHIFT));
                state.editor.key(KeyEvent::new(
                    if redo {
                        KeyCode::Char('y')
                    } else {
                        KeyCode::Char('z')
                    },
                    KeyModifiers::CONTROL,
                ));
                if state.editor.text() != before.text {
                    state.editor = crate::editor::Editor::restore(before).unwrap();
                }
                state.error = Some(error);
            }
        }
        state.trim_history();
    }
}

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    app.hits.clear();
    if area.width < 44 || area.height < 20 {
        app.revision.invalidate_geometry();
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let width = area.width.saturating_sub(4).min(88);
    let inner_width = width.saturating_sub(4);
    app.revision.rendered = true;
    let key = app
        .revision
        .error
        .unwrap_or(if app.revision.confirm_discard {
            "revision-discard-note"
        } else {
            match app.revision.phase {
                Phase::Editing => "revision-note",
                Phase::Ready => "revision-prepared",
                Phase::Done => "revision-started",
                Phase::Retained => "revision-retained",
                Phase::UnknownCopy | Phase::UnknownTurn | Phase::Failed => "revision-unknown",
                Phase::Busy | Phase::Loading => "revision-wait",
            }
        });
    let controls = buttons(app);
    let labels: Vec<_> = controls.iter().map(|c| app.i18n.text(c.label())).collect();
    let mut rows: Vec<Vec<(Command, String, u16, usize)>> = vec![vec![]];
    let mut used = 0;
    for (index, (command, label)) in controls.into_iter().zip(labels).enumerate() {
        let width = (label.width() as u16 + 2).min(inner_width);
        if used > 0 && used + 1 + width > inner_width {
            rows.push(vec![]);
            used = 0;
        }
        if used > 0 {
            used += 1;
        }
        used += width;
        rows.last_mut()
            .unwrap()
            .push((command, label, width, index));
    }
    let mut note = crate::pages::manage::view::note_lines(&app.i18n.text(key), inner_width);
    if !app.revision.confirm_discard
        && !app.revision.show_problem
        && key == "revision-blocked"
        && let Some(problem) = &app.revision.problem
    {
        // Full Host detail stays opt-in and scrollable; it cannot displace the controls.
        let preview = crate::view::safe(problem.text());
        let mut lines = crate::pages::manage::view::note_lines(&preview, inner_width);
        if lines.len() > 2 {
            lines.truncate(2);
        }
        note.extend(lines);
    }
    let note_height = note.len() as u16;
    let footer = rows.len() as u16;
    let fixed = 9 + note_height + footer;
    let available = area.height.saturating_sub(2 + fixed);
    if available == 0 {
        app.revision.invalidate_geometry();
        crate::view::clear_overlay(frame, area);
        frame.render_widget(Paragraph::new(app.i18n.text("terminal-small")), area);
        return;
    }
    let editor = if app.revision.show_problem {
        app.revision.problem.as_mut().unwrap()
    } else {
        &mut app.revision.editor
    };
    let editor_rows = editor
        .preferred_height(width.saturating_sub(8), 15)
        .max(3)
        .min(available);
    let height = fixed + editor_rows;
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    let colors = app.theme.colors();
    let block = Block::bordered()
        .title(app.i18n.text("revision-title"))
        .title_alignment(Alignment::Center)
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .style(base)
        .border_style(Style::default().fg(colors.accent));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    frame.render_widget(block, popup);
    app.revision.rendered = true;
    let editing = app.revision.phase == Phase::Editing
        && !app.revision.confirm_discard
        && !app.revision.show_problem;
    let selected = app.revision.selected;
    let count = app.revision.saved.as_ref().map_or(0, |s| s.inputs.len());
    if app.revision.show_problem {
        frame.render_widget(
            Paragraph::new(app.i18n.text("revision-details"))
                .style(Style::default().fg(colors.warning)),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    } else if count > 0 {
        let label = format!(
            "{}  {} / {}",
            app.i18n.text("revision-input"),
            selected + 1,
            count
        );
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(colors.muted)),
            Rect::new(inner.x + 3, inner.y + 1, inner.width.saturating_sub(15), 1),
        );
        crate::view::button(
            frame,
            app,
            Rect::new(inner.x, inner.y + 1, 2, 1),
            if app.chrome.ascii { "<" } else { "‹" },
            Action::Revision(Command::Select(selected.saturating_sub(1))),
            false,
        );
        crate::view::button(
            frame,
            app,
            Rect::new(inner.right() - 2, inner.y + 1, 2, 1),
            if app.chrome.ascii { ">" } else { "›" },
            Action::Revision(Command::Select(selected + 1)),
            false,
        );
        let display = app.revision.saved.as_ref().unwrap().inputs[selected]
            .content
            .display_text
            .is_some();
        if display {
            let label = app.i18n.text(if app.revision.display {
                "revision-text"
            } else {
                "revision-display"
            });
            let w = (label.width() as u16 + 2).min(inner.width / 2);
            crate::view::button(
                frame,
                app,
                Rect::new(inner.right() - w - 3, inner.y + 1, w, 1),
                &label,
                Action::Revision(Command::Display),
                false,
            );
        }
    }
    let body = Rect::new(inner.x, inner.y + 3, inner.width, editor_rows + 2);
    if !app.revision.show_problem
        && let Some(input) = app
            .revision
            .saved
            .as_ref()
            .and_then(|s| s.inputs.get(selected))
    {
        let content = &input.content;
        let counts = [
            (
                "revision-attachments",
                content.attachments.as_ref().map_or(0, Vec::len),
            ),
            (
                "revision-references",
                content.quotes.as_ref().map_or(0, Vec::len)
                    + content.directory_references.as_ref().map_or(0, Vec::len)
                    + content.inline_references.as_ref().map_or(0, Vec::len),
            ),
            (
                "revision-selections",
                input.original.input_selections.values().map(Vec::len).sum(),
            ),
        ];
        let label = counts
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(key, n)| app.i18n.format(key, &[("count", &n.to_string())]))
            .collect::<Vec<_>>()
            .join(" · ");
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(colors.muted)),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
    }
    let editor_block = Block::bordered().border_style(Style::default().fg(if editing {
        colors.accent
    } else {
        colors.muted
    }));
    let text_area = editor_block.inner(body).inner(Margin::new(1, 0));
    frame.render_widget(editor_block, body);
    if app.revision.show_problem {
        app.revision
            .problem
            .as_mut()
            .unwrap()
            .draw(frame, text_area, false, colors);
    } else if count > 0 {
        app.revision
            .editor
            .draw(frame, text_area, editing && app.revision.focus == 0, colors);
    }
    frame.render_widget(
        Paragraph::new(note)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(if app.revision.error.is_some() {
                colors.warning
            } else {
                colors.muted
            })),
        Rect::new(inner.x, body.bottom() + 1, inner.width, note_height),
    );
    let first = inner.bottom().saturating_sub(rows.len() as u16);
    for (row, buttons) in rows.into_iter().enumerate() {
        let total = buttons.iter().map(|(_, _, w, _)| w).sum::<u16>()
            + buttons.len().saturating_sub(1) as u16;
        let mut x = inner.x + (inner.width - total) / 2;
        for (command, label, width, index) in buttons {
            crate::view::button(
                frame,
                app,
                Rect::new(x, first + row as u16, width, 1),
                &label,
                Action::Revision(command.clone()),
                app.revision.focus == index + 1
                    || (command == Command::Details && app.revision.show_problem),
            );
            x += width + 1;
        }
    }
}
