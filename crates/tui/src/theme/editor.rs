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

mod view;

use super::{Choice, Palette};
use crate::app::{Action, App};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Color;
pub use view::draw;

const SWATCHES: [u32; 18] = [
    0x71a8fd, 0xbe9df7, 0xec939b, 0xe7bd7f, 0x7ecfa4, 0x71ccd1, 0x205fc1, 0x804caf, 0xb33e48,
    0x8c5c13, 0x21714f, 0x147078, 0x131720, 0x202837, 0x647186, 0xa0afc3, 0xe9edf5, 0xf7f8fc,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    Close,
    Save,
    Reload,
    Base(usize),
    Role(usize),
    Swatch(usize),
}
impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Close => "session-cancel",
            Self::Save => "theme-save",
            Self::Reload => "theme-reload",
            _ => "theme-customize",
        }
    }
}

pub struct Editor {
    pub colors: Palette,
    pub chrome: Palette,
    pub name: crate::editor::Editor,
    pub hex: crate::editor::Editor,
    pub role: usize,
    pub focus: usize,
    pub swatch: usize,
    pub base: usize,
    pub visible: bool,
    pub error: Option<&'static str>,
}
impl Editor {
    pub fn new(name: String, colors: Palette) -> Self {
        let mut editor = Self {
            colors,
            chrome: colors,
            name: Default::default(),
            hex: Default::default(),
            role: 3,
            focus: 2,
            swatch: 0,
            base: 0,
            visible: false,
            error: None,
        };
        editor.name.insert(&name);
        editor.sync_hex();
        editor
    }
    fn sync_hex(&mut self) {
        self.hex = Default::default();
        if let Color::Rgb(r, g, b) = self.colors.entries()[self.role].1 {
            self.hex.insert(&format!("#{r:02X}{g:02X}{b:02X}"));
        }
        self.error = None;
    }
    pub fn valid_hex(&mut self) -> bool {
        let text = self.hex.text();
        if text.len() == 7
            && text.starts_with('#')
            && text[1..].bytes().all(|b| b.is_ascii_hexdigit())
            && let Ok(color) = u32::from_str_radix(&text[1..], 16)
        {
            self.colors.set_role(self.role, super::rgb(color));
            self.error = None;
            return true;
        }
        false
    }
    pub fn invalidate(&mut self) {
        self.visible = false;
        self.hex.invalidate_geometry();
        self.name.invalidate_geometry();
    }
}
impl App {
    pub fn theme_action(&mut self, command: Command) {
        match command {
            Command::Open => {
                self.invalidate_editor_geometry();
                self.theme.open_editor(self.i18n.text("theme-custom-name"));
            }
            Command::Close => {
                self.theme.close_editor();
                self.invalidate_editor_geometry();
                self.hits.clear();
            }
            Command::Save => self.theme.save_editor(),
            Command::Reload => self.theme.reload(),
            _ => {
                if self.theme.busy() {
                    return;
                }
                let Some(editor) = &mut self.theme.editor else {
                    return;
                };
                match command {
                    Command::Base(index) if index < 3 => {
                        editor.base = index;
                        editor.focus = 1;
                        editor.colors = [Choice::Maka, Choice::Dusk, Choice::Paper][index].colors();
                        editor.sync_hex();
                    }
                    Command::Role(index) if index < 24 => {
                        editor.role = index;
                        editor.focus = 2;
                        editor.sync_hex();
                    }
                    Command::Swatch(index) if index < SWATCHES.len() => {
                        editor.swatch = index;
                        editor.focus = 3;
                        editor
                            .colors
                            .set_role(editor.role, super::rgb(SWATCHES[index]));
                        editor.sync_hex();
                    }
                    _ => {}
                }
            }
        }
        self.hover = None;
    }

    pub fn theme_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let busy = self.theme.busy();
        let editor = self.theme.editor.as_mut().expect("theme editor");
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc => Some(Command::Close),
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                _ if busy || !editor.visible => None,
                KeyCode::Tab | KeyCode::BackTab => {
                    editor.focus =
                        (editor.focus + if key.code == KeyCode::Tab { 1 } else { 7 }) % 8;
                    return (true, None);
                }
                KeyCode::Enter => match editor.focus {
                    1 => Some(Command::Base(editor.base)),
                    3 => Some(Command::Swatch(editor.swatch)),
                    4 => {
                        if !editor.valid_hex() {
                            editor.error = Some("theme-hex-invalid");
                        }
                        return (true, None);
                    }
                    5 => Some(Command::Reload),
                    6 => Some(Command::Close),
                    7 => Some(Command::Save),
                    _ => None,
                },
                KeyCode::Left | KeyCode::Right if editor.focus == 1 => Some(Command::Base(
                    (editor.base + if key.code == KeyCode::Right { 1 } else { 2 }) % 3,
                )),
                KeyCode::Up
                | KeyCode::Down
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                    if editor.focus == 2 =>
                {
                    Some(Command::Role(match key.code {
                        KeyCode::Home => 0,
                        KeyCode::End => 23,
                        KeyCode::Up => editor.role.saturating_sub(1),
                        KeyCode::Down => (editor.role + 1).min(23),
                        KeyCode::PageUp => editor.role.saturating_sub(10),
                        _ => (editor.role + 10).min(23),
                    }))
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
                    if editor.focus == 3 =>
                {
                    let delta = match key.code {
                        KeyCode::Left => 17,
                        KeyCode::Right => 1,
                        KeyCode::Up => 12,
                        _ => 6,
                    };
                    editor.swatch = (editor.swatch + delta) % SWATCHES.len();
                    return (true, None);
                }
                _ if editor.focus == 0 || editor.focus == 4 => {
                    if matches!(key.code, KeyCode::Char(c) if c.is_control()) {
                        return (false, None);
                    }
                    let changed = if editor.focus == 0 {
                        editor.name.key(key)
                    } else {
                        editor.hex.key(key)
                    };
                    if editor.focus == 4 {
                        editor.valid_hex();
                    }
                    return (changed, None);
                }
                _ => None,
            },
            Event::Paste(text) if !busy && editor.visible && matches!(editor.focus, 0 | 4) => {
                if text.chars().any(char::is_control) {
                    return (false, None);
                }
                let changed = if editor.focus == 0 {
                    editor.name.insert(&text)
                } else {
                    editor.hex.insert(&text)
                };
                if editor.focus == 4 {
                    editor.valid_hex();
                }
                return (changed, None);
            }
            Event::Mouse(mouse) if editor.visible => {
                if !busy {
                    for (field, focus) in [(&mut editor.name, 0), (&mut editor.hex, 4)] {
                        if field.contains((mouse.column, mouse.row).into()) || field.dragging() {
                            editor.focus = focus;
                            return (
                                field.mouse(mouse) || matches!(mouse.kind, MouseEventKind::Down(_)),
                                None,
                            );
                        }
                    }
                    if matches!(
                        mouse.kind,
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    ) && self.hits.iter().any(|hit| {
                        hit.area.contains((mouse.column, mouse.row).into())
                            && matches!(hit.action, Action::Theme(Command::Role(_)))
                    }) {
                        let index = if mouse.kind == MouseEventKind::ScrollUp {
                            editor.role.saturating_sub(1)
                        } else {
                            (editor.role + 1).min(23)
                        };
                        self.theme_action(Command::Role(index));
                        return (true, None);
                    }
                }
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    self.hits
                        .iter()
                        .rev()
                        .find(|hit| hit.area.contains((mouse.column, mouse.row).into()))
                        .and_then(|hit| match &hit.action {
                            Action::Theme(command) => Some(command.clone()),
                            _ => None,
                        })
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(command) = command {
            self.theme_action(command);
            (true, None)
        } else {
            (false, None)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, MouseEvent};
    use ratatui::{Terminal, backend::TestBackend};
    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }
    fn draw(app: &mut App, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::view::draw(frame, app))
            .unwrap();
        assert!(
            app.i18n.diagnostics().is_empty(),
            "{:?}",
            app.i18n.diagnostics()
        );
    }
    #[tokio::test]
    async fn swatches_hex_preview_cancel_and_save_share_real_modal_geometry() {
        for locale in [crate::Locale::En, crate::Locale::ZhCn, crate::Locale::ZhTw] {
            for (width, height) in [(80, 35), (46, 26)] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("theme.json");
                let mut app = App::new(
                    "/unused".into(),
                    crate::i18n::I18n::new(crate::LocalePreference::Explicit(locale), locale),
                );
                app.theme.path = Some(path.clone());
                app.apply(Action::Visit(crate::navigation::Route::Settings));
                let original = app.theme.colors();
                app.apply(Action::Theme(Command::Open));
                draw(&mut app, width, height);
                app.apply(Action::Theme(Command::Role(23)));
                draw(&mut app, width, height);
                assert!(
                    app.hits
                        .iter()
                        .any(|hit| hit.action == Action::Theme(Command::Role(23)))
                );
                app.apply(Action::Theme(Command::Role(3)));
                draw(&mut app, width, height);
                let hit = app
                    .hits
                    .iter()
                    .find(|hit| hit.action == Action::Theme(Command::Swatch(4)))
                    .unwrap()
                    .area;
                app.input(Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: hit.x,
                    row: hit.y,
                    modifiers: KeyModifiers::NONE,
                }));
                assert_eq!(app.theme.colors().accent, super::super::rgb(SWATCHES[4]));
                app.input(key(KeyCode::Tab, KeyModifiers::NONE));
                app.input(key(KeyCode::Char('a'), KeyModifiers::CONTROL));
                app.input(Event::Paste("#AABBCC".into()));
                assert_eq!(app.theme.colors().accent, super::super::rgb(0xaabbcc));
                draw(&mut app, width, height);
                app.input(Event::Mouse(MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 0,
                    row: 0,
                    modifiers: KeyModifiers::NONE,
                }));
                assert!(app.theme.editor.is_none());
                assert_eq!(app.theme.colors(), original);
                assert!(!path.exists());
                assert_eq!(app.navigation.current(), crate::navigation::Route::Settings);
                app.apply(Action::Theme(Command::Open));
                draw(&mut app, 20, 8);
                app.input(key(KeyCode::Enter, KeyModifiers::NONE));
                assert!(!app.theme.busy());
                assert!(!path.exists());
                draw(&mut app, width, height);
                app.apply(Action::Theme(Command::Swatch(5)));
                app.input(key(KeyCode::Tab, KeyModifiers::NONE));
                app.input(key(KeyCode::Char('a'), KeyModifiers::CONTROL));
                app.input(Event::Paste("invalid".into()));
                app.apply(Action::Theme(Command::Save));
                assert!(app.theme.request().is_none());
                assert_eq!(
                    app.theme.editor.as_ref().unwrap().error,
                    Some("theme-hex-invalid")
                );
                app.input(key(KeyCode::Char('a'), KeyModifiers::CONTROL));
                app.input(Event::Paste("#112233".into()));
                app.apply(Action::Theme(Command::Save));
                let request = app.theme.request().unwrap();
                let result = request.execute().await;
                app.theme.complete(request, result);
                assert!(app.theme.editor.is_none());
                assert_eq!(app.theme.choice, Choice::Custom);
                assert_eq!(
                    super::super::custom::read(&path).unwrap().colors.accent,
                    super::super::rgb(0x112233)
                );
                assert_eq!(app.theme.colors().accent, super::super::rgb(0x112233));
            }
        }
    }
}
