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

use super::{Command, Manage};
use crate::app::{Action, App};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};

impl App {
    pub(in crate::pages::manage) fn oauth_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let controls = self.management.oauth.controls();
        let focused = self
            .management
            .dialog
            .as_ref()
            .and_then(|dialog| controls.get(dialog.focus));
        let field = match focused {
            Some(Manage::Oauth(Command::Field(index)))
                if self.oauth_enabled(Command::Field(*index)) =>
            {
                Some(*index)
            }
            _ => None,
        };
        if let Some(index) = field {
            let editor = &mut self.management.oauth.identity.fields[index];
            match &event {
                Event::Paste(text) => {
                    if index < 2 && text.chars().any(char::is_control) {
                        editor.error = Some("oauth-field-invalid");
                    } else {
                        editor.insert(text);
                    }
                    self.management.oauth.error = None;
                    return (true, None);
                }
                Event::Key(key)
                    if key.kind != KeyEventKind::Release
                        && !matches!(
                            key.code,
                            KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc | KeyCode::Enter
                        )
                        && !(key.code == KeyCode::Char('q')
                            && key.modifiers.contains(KeyModifiers::CONTROL)) =>
                {
                    if !matches!(key.code, KeyCode::Char(c) if c.is_control()) {
                        editor.key(*key);
                    }
                    self.management.oauth.error = None;
                    return (true, None);
                }
                _ => {}
            }
        }
        if let Event::Mouse(mouse) = &event {
            let fields = &self.management.oauth.identity.fields;
            let index = fields
                .iter()
                .position(|editor| editor.dragging())
                .or_else(|| {
                    fields
                        .iter()
                        .position(|editor| editor.contains((mouse.column, mouse.row).into()))
                });
            if let Some(index) = index.filter(|index| self.oauth_enabled(Command::Field(*index))) {
                let changed = self.management.oauth.identity.fields[index].mouse(*mouse);
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    self.management.dialog.as_mut().unwrap().focus = controls
                        .iter()
                        .position(|control| *control == Manage::Oauth(Command::Field(index)))
                        .unwrap();
                    return (true, None);
                }
                return (changed, None);
            }
        }
        let dialog = self.management.dialog.as_mut().expect("OAuth dialog");
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                KeyCode::Esc => Some(Manage::Close),
                _ if !dialog.visible => None,
                KeyCode::Tab | KeyCode::Down => {
                    dialog.focus = (dialog.focus + 1) % controls.len();
                    return (true, None);
                }
                KeyCode::BackTab | KeyCode::Up => {
                    dialog.focus = (dialog.focus + controls.len() - 1) % controls.len();
                    return (true, None);
                }
                KeyCode::Enter if field.is_some() => {
                    dialog.focus = (dialog.focus + 1) % controls.len();
                    return (true, None);
                }
                KeyCode::Enter | KeyCode::Char(' ') => controls.get(dialog.focus).cloned(),
                _ => None,
            },
            Event::Mouse(mouse)
                if dialog.visible && mouse.kind == MouseEventKind::Down(MouseButton::Left) =>
            {
                self.hits
                    .iter()
                    .rev()
                    .find(|hit| hit.area.contains((mouse.column, mouse.row).into()))
                    .and_then(|hit| match &hit.action {
                        Action::Manage(command @ (Manage::Oauth(_) | Manage::Close)) => {
                            Some(command.clone())
                        }
                        _ => None,
                    })
            }
            _ => None,
        };
        let effect = command.and_then(|command| self.apply(Action::Manage(command)));
        // Consume modal input even if the focused control is disabled.
        (true, effect)
    }
}
