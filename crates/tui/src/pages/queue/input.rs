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
use super::{Command, Kind};
use crate::app::{Action, App, Focus};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::Position;

impl App {
    pub fn queue_key(&mut self, key: KeyEvent) -> (bool, Option<Action>) {
        let target = self.queue_selected().map(|row| row.target);
        let command = match key.code {
            KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => {
                self.focus = Focus::Composer;
                return (true, None);
            }
            KeyCode::Up | KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                target.map(|target| Command::Reorder(target, key.code == KeyCode::Down))
            }
            KeyCode::Up | KeyCode::Down => {
                self.queue_move(key.code == KeyCode::Down);
                return (true, None);
            }
            KeyCode::Home | KeyCode::End => {
                let rows = self.queue_rows();
                self.queue.selected = if key.code == KeyCode::Home {
                    rows.first()
                } else {
                    rows.last()
                }
                .map(|row| row.target.entry.clone());
                return (true, None);
            }
            KeyCode::Char('e') | KeyCode::Enter => target.map(Command::Edit),
            KeyCode::Char('x') | KeyCode::Delete => target.map(Command::Retract),
            KeyCode::Char('s') if key.modifiers.is_empty() => target.map(Command::Promote),
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return (true, self.apply(Action::Palette));
            }
            KeyCode::F(11) => return (true, self.apply(Action::ToggleFullscreen)),
            _ => return (false, None),
        };
        (
            true,
            command.and_then(|command| self.apply(Action::Queue(command))),
        )
    }
    pub fn queue_edit_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let action = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Esc {
                    Some(Action::Queue(Command::Close))
                } else if key.code == KeyCode::Char('q')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    Some(Action::Quit)
                } else if self.queue.edit_area.is_none() {
                    return (false, None);
                } else if key.code == KeyCode::Char('s')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    Some(Action::Queue(Command::Save))
                } else {
                    return (
                        self.queue
                            .edit
                            .as_mut()
                            .is_some_and(|edit| edit.editor.key(key)),
                        None,
                    );
                }
            }
            Event::Paste(text) if self.queue.edit_area.is_some() => {
                return (
                    self.queue
                        .edit
                        .as_mut()
                        .is_some_and(|edit| edit.editor.insert(&text)),
                    None,
                );
            }
            Event::Mouse(mouse) if self.queue.edit_area.is_some() => {
                let point = Position::new(mouse.column, mouse.row);
                if let Some(edit) = &mut self.queue.edit
                    && (edit.editor.contains(point) || edit.editor.dragging())
                    && edit.editor.mouse(mouse)
                {
                    return (true, None);
                }
                if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
                    return (false, None);
                }
                self.hits
                    .iter()
                    .rev()
                    .find(|hit| hit.area.contains(point))
                    .and_then(|hit| {
                        matches!(hit.action, Action::Queue(Command::Save | Command::Close))
                            .then(|| hit.action.clone())
                    })
            }
            _ => return (false, None),
        };
        (true, action.and_then(|action| self.apply(action)))
    }
    pub fn queue_edit_status(&self) -> Option<String> {
        let edit = self.queue.edit.as_ref()?;
        if let Some((target, key, error)) = &self.queue.error
            && target == &edit.target
        {
            return Some(
                self.i18n
                    .format(key, &[("error", &crate::view::safe(error))]),
            );
        }
        if self.queue_busy() {
            return Some(self.i18n.text("queue-saving"));
        }
        if self
            .queue_row(&edit.target)
            .is_none_or(|row| row.kind == Kind::InFlight)
        {
            return Some(self.i18n.text("queue-changed"));
        }
        if let Some(error) = edit.editor.error {
            return Some(self.i18n.text(error));
        }
        None
    }
}
