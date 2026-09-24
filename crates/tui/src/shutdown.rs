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
    ui::{Role, Sheet, Tone},
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
}
pub(crate) enum Prompt {
    Busy,
    Failed(String),
}
impl State {
    pub fn show(&mut self, prompt: Prompt) {
        self.stopping = false;
        self.prompt = Some(prompt);
    }
}

/// Cancel stays the default: quitting with work in flight takes a choice.
pub(crate) fn sheet(app: &App) -> Option<Sheet<Action>> {
    let prompt = app.shutdown.prompt.as_ref()?;
    let (key, mut message) = match prompt {
        Prompt::Busy => ("shutdown-busy", app.i18n.text("shutdown-busy")),
        Prompt::Failed(_) => ("shutdown-failed", app.i18n.text("shutdown-failed")),
    };
    if let Prompt::Failed(error) = prompt {
        message.push_str("\n\n");
        message.push_str(&error.chars().take(256).collect::<String>());
    }
    let mut sheet = Sheet::new(key, app.i18n.text("command-quit"))
        .text("message", &message, Tone::Normal)
        .button(
            "cancel",
            app.i18n.text("shutdown-cancel"),
            Role::Normal,
            Action::CancelQuit,
            true,
        )
        .button(
            "detach",
            app.i18n.text("command-detach"),
            Role::Normal,
            Action::Detach,
            true,
        );
    if matches!(prompt, Prompt::Busy) {
        sheet = sheet.button(
            "force",
            app.i18n.text("shutdown-force"),
            Role::Destructive,
            Action::ConfirmQuit,
            app.enabled(&Action::ConfirmQuit),
        );
    }
    Some(sheet.focus("cancel"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        i18n::{I18n, Locale, LocalePreference},
        navigation::Route,
    };
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend};

    /// Draws a frame and returns where `label` appears, if it does.
    fn frame(app: &mut App, width: u16, height: u16, label: &str) -> Option<(u16, u16)> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::view::draw(frame, app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height).find_map(|y| {
            // Skip the cells a wide glyph covers, so columns stay in cells.
            let (mut line, mut x) = (String::new(), 0);
            while x < width {
                let symbol = buffer[(x, y)].symbol();
                line.push_str(symbol);
                x += (unicode_width::UnicodeWidthStr::width(symbol) as u16).max(1);
            }
            line.find(label).map(|byte| {
                (
                    unicode_width::UnicodeWidthStr::width(&line[..byte]) as u16,
                    y,
                )
            })
        })
    }
    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn mouse(kind: MouseEventKind, (x, y): (u16, u16)) -> Event {
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
            let force = app.i18n.text("shutdown-force");
            app.apply(Action::Visit(Route::Session("draft".into())));
            app.drafts.get_mut("draft").unwrap().insert("unsent draft");
            app.shutdown.show(Prompt::Busy);
            let at = frame(&mut app, 80, 24, &force).expect("force quit is offered");
            app.input(mouse(MouseEventKind::Moved, at));
            assert_eq!(
                app.input(key(KeyCode::Enter)).1,
                None,
                "hover must not arm Enter"
            );
            assert!(app.shutdown.prompt.is_none());
            app.shutdown.show(Prompt::Busy);
            frame(&mut app, 80, 24, &force);
            let click = mouse(MouseEventKind::Down(MouseButton::Left), at);
            assert_eq!(app.input(click.clone()).1, Some(Action::ConfirmQuit));
            app.input(Event::Resize(24, 6));
            assert!(app.input(key(KeyCode::Enter)).1.is_none());
            assert!(frame(&mut app, 24, 6, &force).is_none());
            assert!(app.input(key(KeyCode::Enter)).1.is_none());
            assert!(app.input(click).1.is_none(), "no stale geometry");
            frame(&mut app, 80, 24, &force);
            app.input(mouse(MouseEventKind::Down(MouseButton::Left), (0, 0)));
            assert!(app.shutdown.prompt.is_none(), "outside cancels");
            assert_eq!(app.drafts["draft"].text(), "unsent draft");
            app.shutdown.show(Prompt::Failed("uncertain result".into()));
            assert!(frame(&mut app, 80, 24, &force).is_none());
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
