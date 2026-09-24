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

use super::{Action, App, Command};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};

impl App {
    pub fn onboarding_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let page_size = self
            .hits
            .iter()
            .filter(|h| matches!(h.action, Action::Onboard(Command::Toggle(_))))
            .count()
            .max(1);
        let f = self.onboarding.dialog.as_mut().expect("onboarding form");
        let editable =
            f.visible && !f.blocked && !f.providers.is_empty() && self.onboarding.pending.is_none();
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                KeyCode::Esc => Some(Command::Close),
                _ if !editable => None,
                KeyCode::Tab => {
                    f.focus = (f.focus + 1) % f.count();
                    return (true, None);
                }
                KeyCode::BackTab => {
                    f.focus = (f.focus + f.count() - 1) % f.count();
                    return (true, None);
                }
                KeyCode::Enter => Some(if f.models.is_some() {
                    match f.focus {
                        1 => Command::Back,
                        2 => Command::Close,
                        _ => Command::Save,
                    }
                } else {
                    match f.focus {
                        0 => Command::Provider,
                        5 => Command::Close,
                        _ => Command::Verify,
                    }
                }),
                KeyCode::Up | KeyCode::Down if f.models.is_some() && f.focus == 0 => {
                    let n = f.models.as_ref().unwrap().len();
                    f.row = if key.code == KeyCode::Up {
                        f.row.saturating_sub(1)
                    } else {
                        (f.row + 1).min(n.saturating_sub(1))
                    };
                    return (true, None);
                }
                KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown
                    if f.models.is_some() && f.focus == 0 =>
                {
                    let last = f.models.as_ref().unwrap().len().saturating_sub(1);
                    f.row = match key.code {
                        KeyCode::Home => 0,
                        KeyCode::End => last,
                        KeyCode::PageUp => f.row.saturating_sub(page_size),
                        _ => f.row.saturating_add(page_size).min(last),
                    };
                    return (true, None);
                }
                KeyCode::Char(' ') if f.models.is_some() && f.focus == 0 => f
                    .models
                    .as_ref()
                    .and_then(|m| m.get(f.row))
                    .map(|m| Command::Toggle(m.id.clone())),
                _ if f.models.is_none() && (1..=3).contains(&f.focus) => {
                    if matches!(key.code,KeyCode::Char(c) if c.is_control()) {
                        return (false, None);
                    }
                    let changed = f.fields[f.focus - 1].key(key);
                    if changed {
                        f.error = None;
                    }
                    return (changed, None);
                }
                _ => None,
            },
            Event::Paste(text) if editable && f.models.is_none() && (1..=3).contains(&f.focus) => {
                if f.focus != 2 && text.chars().any(char::is_control) {
                    f.error = Some("onboard-field-invalid");
                    return (true, None);
                }
                let changed = f.fields[f.focus - 1].insert(&text);
                f.error = None;
                return (changed, None);
            }
            Event::Mouse(mouse) if f.visible => {
                if matches!(
                    mouse.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                ) && editable
                    && let Some(models) = &f.models
                    && self.hits.iter().any(|h| {
                        h.area.contains((mouse.column, mouse.row).into())
                            && matches!(h.action, Action::Onboard(Command::Toggle(_)))
                    })
                {
                    let n = models.len();
                    f.row = if mouse.kind == MouseEventKind::ScrollUp {
                        f.row.saturating_sub(1)
                    } else {
                        (f.row + 1).min(n.saturating_sub(1))
                    };
                    f.focus = 0;
                    return (true, None);
                }
                if editable && f.models.is_none() {
                    for (index, editor) in f.fields.iter_mut().enumerate() {
                        if editor.contains((mouse.column, mouse.row).into()) || editor.dragging() {
                            f.focus = index + 1;
                            let changed = editor.mouse(mouse);
                            return (
                                changed || matches!(mouse.kind, MouseEventKind::Down(_)),
                                None,
                            );
                        }
                    }
                }
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    self.hits
                        .iter()
                        .rev()
                        .find(|h| h.area.contains((mouse.column, mouse.row).into()))
                        .and_then(|h| match &h.action {
                            Action::Onboard(c) => Some(c.clone()),
                            _ => None,
                        })
                } else {
                    None
                }
            }
            _ => None,
        };
        (
            command.is_some(),
            command.and_then(|c| self.apply(Action::Onboard(c))),
        )
    }
}
