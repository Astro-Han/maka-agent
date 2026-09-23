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

use super::{Choice, Palette};
use ratatui::style::Color;
use serde::{Deserialize, Deserializer};
use std::{
    io::{Read, Write},
    path::Path,
};
use unicode_width::UnicodeWidthStr;

const MAX_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Missing,
    File,
    TooLarge,
    Invalid { line: usize, column: usize },
    Version,
    Name,
    Changed,
    Save,
}

#[derive(Debug, Clone)]
pub struct Custom {
    pub name: String,
    pub colors: Palette,
    pub source: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    name: String,
    #[serde(default)]
    base: Base,
    #[serde(default)]
    colors: Colors,
    #[serde(default)]
    syntax: Syntax,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Base {
    #[default]
    Maka,
    Dusk,
    Paper,
}

#[derive(Clone, Copy)]
struct Rgb(Color);
impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() != 7
            || !text.starts_with('#')
            || !text[1..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(serde::de::Error::custom("expected #RRGGBB"));
        }
        let hex = u32::from_str_radix(&text[1..], 16).map_err(serde::de::Error::custom)?;
        Ok(Self(super::rgb(hex)))
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
struct Colors {
    background: Option<Rgb>,
    foreground: Option<Rgb>,
    surface: Option<Rgb>,
    accent: Option<Rgb>,
    thinking: Option<Rgb>,
    success: Option<Rgb>,
    warning: Option<Rgb>,
    error: Option<Rgb>,
    muted: Option<Rgb>,
    subtle: Option<Rgb>,
    border: Option<Rgb>,
    scrollbar: Option<Rgb>,
    selection: Option<Rgb>,
    selection_text: Option<Rgb>,
    search: Option<Rgb>,
    search_active: Option<Rgb>,
    search_text: Option<Rgb>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Syntax {
    keyword: Option<Rgb>,
    string: Option<Rgb>,
    comment: Option<Rgb>,
    number: Option<Rgb>,
    function: Option<Rgb>,
    r#type: Option<Rgb>,
    operator: Option<Rgb>,
}

fn decode(bytes: &[u8]) -> Result<Custom, Error> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    let file: File = serde_json::from_slice(bytes).map_err(|e| Error::Invalid {
        line: e.line(),
        column: e.column(),
    })?;
    if file.version != 1 {
        return Err(Error::Version);
    }
    if file.name.trim().width() == 0
        || file.name.chars().count() > 40
        || file.name.chars().any(|c| {
            c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
    {
        return Err(Error::Name);
    }
    let mut colors = match file.base {
        Base::Maka => Choice::Maka,
        Base::Dusk => Choice::Dusk,
        Base::Paper => Choice::Paper,
    }
    .colors();
    macro_rules! apply {
        ($($field:ident),+ $(,)?) => {
            $(if let Some(color) = file.colors.$field { colors.$field = color.0; })+
        };
    }
    apply!(
        background,
        foreground,
        surface,
        accent,
        thinking,
        success,
        warning,
        error,
        muted,
        subtle,
        border,
        scrollbar,
        selection,
        selection_text,
        search,
        search_active,
        search_text
    );
    for (target, value) in colors.syntax.iter_mut().zip([
        file.syntax.keyword,
        file.syntax.string,
        file.syntax.comment,
        file.syntax.number,
        file.syntax.function,
        file.syntax.r#type,
        file.syntax.operator,
    ]) {
        if let Some(color) = value {
            *target = color.0;
        }
    }
    Ok(Custom {
        name: file.name.trim().into(),
        colors,
        source: bytes.to_vec(),
    })
}

pub fn read(path: &Path) -> Result<Custom, Error> {
    decode(&read_bytes(path)?)
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, Error> {
    let map_io = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::Missing
        } else {
            Error::File
        }
    };
    let meta = path.symlink_metadata().map_err(map_io)?;
    if !meta.is_file() {
        return Err(Error::File);
    }
    if meta.len() > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(map_io)?
    };
    #[cfg(windows)]
    let file = maka_event_log::root::windows::open_nofollow(path, false).map_err(map_io)?;
    if !file.metadata().map_err(map_io)?.is_file() {
        return Err(Error::File);
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(map_io)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::TooLarge);
    }
    Ok(bytes)
}

pub fn encode(name: &str, colors: Palette) -> Result<Vec<u8>, Error> {
    let mut ui = serde_json::Map::new();
    let mut syntax = serde_json::Map::new();
    for (index, (key, value)) in colors.entries().into_iter().enumerate() {
        let Color::Rgb(r, g, b) = value else {
            return Err(Error::Save);
        };
        let target = if index < 17 { &mut ui } else { &mut syntax };
        target.insert(key.into(), format!("#{r:02x}{g:02x}{b:02x}").into());
    }
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "version": 1, "name": name, "base": "maka", "colors": ui, "syntax": syntax,
    }))
    .map_err(|_| Error::Save)?;
    decode(&bytes)?;
    Ok(bytes)
}

pub fn save(path: &Path, expected: Option<&[u8]>, bytes: &[u8]) -> Result<Custom, Error> {
    let theme = decode(bytes)?;
    let check = || match read_bytes(path) {
        Ok(current) if Some(current.as_slice()) == expected => Ok(()),
        Err(Error::Missing) if expected.is_none() => Ok(()),
        _ => Err(Error::Changed),
    };
    check()?;
    let parent = path.parent().ok_or(Error::Save)?;
    std::fs::create_dir_all(parent).map_err(|_| Error::Save)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Save)?;
    temp.write_all(bytes).map_err(|_| Error::Save)?;
    temp.as_file().sync_all().map_err(|_| Error::Save)?;
    check()?; // Detect ordinary external edits before atomic replacement.
    temp.persist(path).map_err(|_| Error::Save)?;
    Ok(theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_file_is_strict_bounded_data_and_never_follows_special_files() {
        let source = br##"{"version":1,"name":"  Aurora  ","base":"paper",
            "colors":{"accent":"#123456","selection-text":"#ffffff"},
            "syntax":{"keyword":"#654321"}}"##;
        let custom = decode(source).unwrap();
        assert_eq!(custom.name, "Aurora");
        assert_eq!(custom.colors.accent, super::super::rgb(0x123456));
        assert_eq!(custom.colors.foreground, Choice::Paper.colors().foreground);
        assert_eq!(custom.colors.syntax[0], super::super::rgb(0x654321));
        for source in [
            r#"{"version":1,"name":"x","base":"terminal"}"#,
            r#"{"version":1,"name":"x","script":"echo nope"}"#,
            r##"{"version":1,"name":"x","colors":{"accent":"#fff"}}"##,
            r##"{"version":1,"name":"x","colors":{"typo":"#123456"}}"##,
            r##"{"version":1,"name":"x","syntax":{"keyword":"#123456","keyword":"#abcdef"}}"##,
            r#"{"version":1,"version":1,"name":"x"}"#,
        ] {
            assert!(matches!(
                decode(source.as_bytes()),
                Err(Error::Invalid { .. })
            ));
        }
        assert_eq!(
            decode(br#"{"version":2,"name":"x"}"#).unwrap_err(),
            Error::Version
        );
        assert_eq!(
            decode(br#"{"version":1,"name":"\u001b"}"#).unwrap_err(),
            Error::Name
        );
        assert_eq!(
            decode(&vec![b' '; MAX_BYTES as usize + 1]).unwrap_err(),
            Error::TooLarge
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.json");
        assert_eq!(read(&path).unwrap_err(), Error::Missing);
        std::fs::write(&path, source).unwrap();
        assert_eq!(read(&path).unwrap().colors, custom.colors);
        assert_eq!(std::fs::read(&path).unwrap(), source);
        let encoded = encode("Saved", custom.colors).unwrap();
        assert_eq!(save(&path, None, &encoded).unwrap_err(), Error::Changed);
        assert_eq!(std::fs::read(&path).unwrap(), source);
        assert_eq!(
            save(&path, Some(source), &encoded).unwrap().colors,
            custom.colors
        );
        assert_eq!(read(&path).unwrap().name, "Saved");
        assert_eq!(read(dir.path()).unwrap_err(), Error::File);
        #[cfg(unix)]
        {
            let link = dir.path().join("link.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert_eq!(read(&link).unwrap_err(), Error::File);
            use std::os::unix::ffi::OsStrExt;
            let fifo = dir.path().join("pipe");
            let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            assert_eq!(read(&fifo).unwrap_err(), Error::File);
        }
    }
}
