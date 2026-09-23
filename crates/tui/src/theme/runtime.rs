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

use super::{
    Choice, Palette,
    custom::{self, Custom, Error},
};
use crate::i18n::I18n;
use std::path::PathBuf;

#[derive(Default)]
pub struct Theme {
    pub choice: Choice,
    pub path: Option<PathBuf>,
    pub error: Option<Error>,
    pub editor: Option<super::editor::Editor>,
    custom: Option<Custom>,
    requested: Option<bool>,
    loading: bool,
    generation: u64,
    write: Option<Vec<u8>>,
}

pub struct Request {
    path: PathBuf,
    generation: u64,
    activate: bool,
    write: Option<Vec<u8>>,
    expected: Option<Vec<u8>>,
    quiet_missing: bool,
    editing: bool,
}
impl Request {
    pub async fn execute(&self) -> Result<Custom, Error> {
        let path = self.path.clone();
        let write = self.write.clone();
        let expected = self.expected.clone();
        tokio::task::spawn_blocking(move || match write {
            Some(bytes) => custom::save(&path, expected.as_deref(), &bytes),
            None => custom::read(&path),
        })
        .await
        .unwrap_or(Err(Error::File))
    }
}

impl Theme {
    pub fn from_environment() -> Self {
        let path: Result<PathBuf, Error> = (|| {
            if let Some(path) = std::env::var_os("MAKA_TUI_THEME") {
                return Ok(PathBuf::from(path));
            }
            let base = match std::env::var_os("MAKA_TUI_STATE_DIR") {
                Some(base) => PathBuf::from(base),
                None => maka_event_log::root::RootNamespaces::for_current_account()
                    .map_err(|_| Error::File)?
                    .ownership
                    .parent()
                    .ok_or(Error::File)?
                    .join("tui"),
            };
            Ok(base.join("theme.json"))
        })();
        match path {
            Ok(path) if path.is_absolute() => Self {
                path: Some(path),
                requested: Some(false),
                ..Self::default()
            },
            _ => Self {
                error: Some(Error::File),
                ..Self::default()
            },
        }
    }
    pub fn colors(&self) -> Palette {
        if let Some(editor) = &self.editor {
            return editor.colors;
        }
        if self.choice == Choice::Custom {
            self.custom
                .as_ref()
                .map_or_else(Palette::default, |custom| custom.colors)
        } else {
            self.choice.colors()
        }
    }
    pub fn title(&self, i18n: &I18n) -> String {
        if self.choice == Choice::Custom {
            match &self.custom {
                Some(custom) => i18n.format("palette-custom", &[("name", &custom.name)]),
                None => i18n.text("palette-custom-fallback"),
            }
        } else {
            i18n.text(self.choice.label())
        }
    }
    pub fn cycle(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.choice = if self.choice == Choice::Terminal && self.custom.is_some() {
            Choice::Custom
        } else {
            self.choice.next()
        };
    }
    pub fn busy(&self) -> bool {
        self.loading || self.requested.is_some()
    }
    pub fn open_editor(&mut self, default_name: String) {
        if self.busy() {
            return;
        }
        let colors = if self.choice == Choice::Terminal {
            Palette::default()
        } else {
            self.colors()
        };
        let name = self
            .custom
            .as_ref()
            .filter(|_| self.choice == Choice::Custom)
            .map_or(default_name, |custom| custom.name.clone());
        self.generation = self.generation.wrapping_add(1);
        self.editor = Some(super::editor::Editor::new(name, colors));
    }
    pub fn close_editor(&mut self) {
        self.editor = None;
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn save_editor(&mut self) {
        if self.busy() {
            return;
        }
        let Some(editor) = &mut self.editor else {
            return;
        };
        if !editor.valid_hex() {
            editor.error = Some("theme-hex-invalid");
            return;
        }
        match custom::encode(editor.name.text(), editor.colors) {
            Ok(bytes) => {
                self.write = Some(bytes);
                self.requested = Some(true);
            }
            Err(error) => {
                editor.error = Some(if error == Error::Name {
                    "theme-file-name"
                } else {
                    "theme-file-save"
                })
            }
        }
    }
    pub fn reload(&mut self) {
        if !self.busy() {
            self.requested = Some(self.editor.is_none());
        }
    }
    pub fn request(&mut self) -> Option<Request> {
        if self.loading {
            return None;
        }
        let activate = self.requested.take()?;
        let Some(path) = &self.path else {
            self.error = Some(Error::File);
            self.write = None;
            return None;
        };
        self.loading = true;
        Some(Request {
            path: path.clone(),
            generation: self.generation,
            activate,
            write: self.write.take(),
            expected: self.custom.as_ref().map(|custom| custom.source.clone()),
            quiet_missing: !activate && self.editor.is_none(),
            editing: self.editor.is_some(),
        })
    }
    pub fn complete(&mut self, request: Request, result: Result<Custom, Error>) {
        self.loading = false;
        if request.editing && request.generation != self.generation {
            return; // Dismissed editor: a late result cannot change its restored preview.
        }
        match result {
            Ok(custom) => {
                if request.generation == self.generation {
                    if request.write.is_some() {
                        self.editor = None;
                    } else if let Some(editor) = &mut self.editor {
                        let chrome = editor.chrome;
                        *editor = super::editor::Editor::new(custom.name.clone(), custom.colors);
                        editor.chrome = chrome;
                    }
                }
                self.custom = Some(custom);
                self.error = None;
                if request.activate && request.generation == self.generation {
                    self.choice = Choice::Custom;
                }
            }
            Err(Error::Missing) if request.quiet_missing && self.choice != Choice::Custom => {
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn error_text(&self, i18n: &I18n) -> Option<String> {
        self.error.as_ref().map(|error| match error {
            Error::Missing => i18n.text("theme-file-missing"),
            Error::File => i18n.text("theme-file-unreadable"),
            Error::TooLarge => i18n.text("theme-file-large"),
            Error::Version => i18n.text("theme-file-version"),
            Error::Name => i18n.text("theme-file-name"),
            Error::Changed => i18n.text("theme-file-changed"),
            Error::Save => i18n.text("theme-file-save"),
            Error::Invalid { line, column } => i18n.format(
                "theme-file-invalid",
                &[("line", &line.to_string()), ("column", &column.to_string())],
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn reload_is_atomic_preserves_last_good_colors_and_cannot_override_later_choice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.json");
        std::fs::write(&path, br#"{"version":1,"name":"Custom","base":"paper"}"#).unwrap();
        let mut theme = Theme {
            path: Some(path.clone()),
            ..Theme::default()
        };
        theme.reload();
        let request = theme.request().unwrap();
        assert!(theme.busy());
        assert!(theme.request().is_none());
        let result = request.execute().await;
        theme.complete(request, result);
        assert_eq!(theme.choice, Choice::Custom);
        assert_eq!(theme.colors(), Choice::Paper.colors());
        std::fs::write(&path, b"broken").unwrap();
        theme.reload();
        let request = theme.request().unwrap();
        let result = request.execute().await;
        theme.complete(request, result);
        assert_eq!(theme.colors(), Choice::Paper.colors());
        assert!(theme.error.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        std::fs::write(&path, br#"{"version":1,"name":"Custom","base":"dusk"}"#).unwrap();
        theme.reload();
        let request = theme.request().unwrap();
        theme.cycle();
        let result = request.execute().await;
        theme.complete(request, result);
        assert_eq!(
            theme.choice,
            Choice::Maka,
            "late load must not undo user selection"
        );
        assert!(theme.error.is_none());
        theme.cycle();
        theme.cycle();
        theme.cycle();
        theme.cycle();
        assert_eq!(theme.choice, Choice::Custom);
        assert_eq!(theme.colors(), Choice::Dusk.colors());
        theme.open_editor("Preview".into());
        std::fs::write(&path, br#"{"version":1,"name":"Late","base":"paper"}"#).unwrap();
        theme.reload();
        let request = theme.request().unwrap();
        theme.close_editor();
        let result = request.execute().await;
        theme.complete(request, result);
        assert_eq!(
            theme.colors(),
            Choice::Dusk.colors(),
            "cancel restores colors even if reload finishes later"
        );
    }
}
