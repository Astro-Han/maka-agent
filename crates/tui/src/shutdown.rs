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
    app::{Action, App},
    pages::manage::view::note_lines,
};
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::Style,
    widgets::{Block, BorderType, Paragraph},
};
use std::path::PathBuf;

/// A frozen local Host lifetime. Never stop a replacement discovered later.
#[derive(Clone)]
pub struct ShutdownRequest {
    pub root: PathBuf,
    pub identity: maka_client::HostIdentity,
    pub interrupt: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Stopped,
    Busy,
}
#[derive(Default)]
pub(crate) struct State {
    pub stopping: bool,
    pub prompt: Option<Prompt>,
    focus: usize,
    pub visible: bool,
}
pub(crate) enum Prompt {
    Busy,
    Failed(String),
}
impl State {
    pub fn show(&mut self, prompt: Prompt) {
        self.stopping = false;
        self.prompt = Some(prompt);
        self.focus = 0;
        self.visible = false;
    }
    fn actions(&self) -> Vec<(Action, &'static str)> {
        let mut actions = vec![
            (Action::CancelQuit, "shutdown-cancel"),
            (Action::Detach, "command-detach"),
        ];
        if matches!(self.prompt, Some(Prompt::Busy)) {
            actions.push((Action::ConfirmQuit, "shutdown-force"));
        }
        actions
    }
}

impl App {
    pub(crate) fn shutdown_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let actions = self.shutdown.actions();
        let action = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc => Some(Action::CancelQuit),
                _ if !self.shutdown.visible => None,
                KeyCode::Tab | KeyCode::Right | KeyCode::Down => {
                    self.shutdown.focus = (self.shutdown.focus + 1) % actions.len();
                    return (true, None);
                }
                KeyCode::BackTab | KeyCode::Left | KeyCode::Up => {
                    self.shutdown.focus = (self.shutdown.focus + actions.len() - 1) % actions.len();
                    return (true, None);
                }
                KeyCode::Enter => actions
                    .get(self.shutdown.focus)
                    .map(|(action, _)| action.clone()),
                _ => None,
            },
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                let point = (mouse.column, mouse.row).into();
                if self.modal_area.is_some_and(|area| !area.contains(point)) {
                    Some(Action::CancelQuit)
                } else if self.shutdown.visible {
                    self.hits
                        .iter()
                        .rev()
                        .find(|hit| hit.area.contains(point))
                        .map(|hit| hit.action.clone())
                        .filter(|action| actions.iter().any(|(candidate, _)| candidate == action))
                } else {
                    None
                }
            }
            _ => None,
        };
        (
            action.is_some(),
            action.and_then(|action| self.apply(action)),
        )
    }
}

pub(crate) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    app.hits.clear();
    let Some(prompt) = &app.shutdown.prompt else {
        return;
    };
    let width = area.width.saturating_sub(2).min(64);
    let mut message = app.i18n.text(match prompt {
        Prompt::Busy => "shutdown-busy",
        Prompt::Failed(_) => "shutdown-failed",
    });
    if let Prompt::Failed(error) = prompt {
        message.push_str("\n\n");
        message.push_str(&crate::view::safe(
            &error.chars().take(256).collect::<String>(),
        ));
    }
    let lines = note_lines(&message, width.saturating_sub(4));
    let height = lines.len() as u16 + 6;
    if width < 36 || height > area.height.saturating_sub(2) {
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small"))
                .alignment(Alignment::Center)
                .style(base),
            area,
        );
        return;
    }
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    app.modal_area = Some(popup);
    app.shutdown.visible = true;
    crate::view::clear_overlay(frame, popup);
    let block = Block::bordered()
        .title(app.i18n.text("command-quit"))
        .title_alignment(Alignment::Center)
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(app.theme.colors().warning))
        .style(base);
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(inner.x, inner.y + 1, inner.width, height - 5),
    );
    let actions = app.shutdown.actions();
    let cell = inner.width / actions.len() as u16;
    for (index, (action, label)) in actions.into_iter().enumerate() {
        crate::view::button(
            frame,
            app,
            Rect::new(inner.x + index as u16 * cell, inner.bottom() - 1, cell, 1),
            &app.i18n.text(label),
            action,
            app.shutdown.focus == index,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        i18n::{I18n, Locale, LocalePreference},
        navigation::Route,
    };
    use crossterm::event::{KeyEvent, KeyModifiers, MouseEvent};
    use ratatui::{Terminal, backend::TestBackend};

    fn frame(app: &mut App, width: u16, height: u16) {
        Terminal::new(TestBackend::new(width, height))
            .unwrap()
            .draw(|frame| crate::view::draw(frame, app))
            .unwrap();
    }
    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn confirmation_defaults_to_cancel_and_never_uses_hidden_or_stale_controls() {
        for locale in [Locale::En, Locale::ZhCn, Locale::ZhTw] {
            let mut app = App::new(
                "/unused".into(),
                I18n::new(LocalePreference::Explicit(locale), locale),
            );
            app.apply(Action::Visit(Route::Session("draft".into())));
            app.drafts.get_mut("draft").unwrap().insert("unsent draft");
            app.shutdown.show(Prompt::Busy);
            frame(&mut app, 80, 24);
            assert!(app.shutdown.visible);
            app.input(key(KeyCode::Enter));
            assert!(app.shutdown.prompt.is_none());
            app.shutdown.show(Prompt::Busy);
            frame(&mut app, 80, 24);
            let force = app
                .hits
                .iter()
                .find(|hit| hit.action == Action::ConfirmQuit)
                .unwrap()
                .area;
            assert!(app.input(mouse(MouseEventKind::Moved, force.x, force.y)).0);
            assert_eq!(app.hover, Some(Action::ConfirmQuit));
            assert_eq!(app.shutdown.focus, 0, "hover must not arm Enter");
            assert_eq!(
                app.input(mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    force.x,
                    force.y
                ))
                .1,
                Some(Action::ConfirmQuit)
            );
            app.input(Event::Resize(24, 6));
            assert!(app.input(key(KeyCode::Enter)).1.is_none());
            frame(&mut app, 24, 6);
            assert!(!app.shutdown.visible);
            assert!(app.input(key(KeyCode::Enter)).1.is_none());
            assert!(
                app.input(mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    force.x,
                    force.y
                ))
                .1
                .is_none()
            );
            frame(&mut app, 80, 24);
            app.input(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
            assert!(app.shutdown.prompt.is_none());
            assert_eq!(app.drafts["draft"].text(), "unsent draft");
            app.shutdown.show(Prompt::Failed("uncertain result".into()));
            frame(&mut app, 80, 24);
            assert!(!app.hits.iter().any(|hit| hit.action == Action::ConfirmQuit));
            assert!(!app.enabled(&Action::ConfirmQuit));
            app.input(key(KeyCode::Tab));
            assert_eq!(app.input(key(KeyCode::Enter)).1, Some(Action::Detach));
            app.shutdown = State {
                stopping: true,
                ..Default::default()
            };
            app.closing = true;
            app.input(key(KeyCode::Esc));
            assert!(
                app.closing,
                "an admitted shutdown cannot be cancelled by hiding its UI"
            );
        }
    }
}
