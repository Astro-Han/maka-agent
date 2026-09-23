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
    navigation::Route,
    pages::sessions::Detail,
    theme::Palette,
};
use ratatui::text::{Line, Span};
use std::{ops::Range, path::Path};
use unicode_width::UnicodeWidthStr;

pub struct Link {
    pub source: Range<usize>,
    pub path: String,
}

pub fn valid_path(path: &str) -> bool {
    !path.trim().is_empty()
        && path.len() <= 4096
        && !path.contains("://")
        && !path.chars().any(|ch| ch.is_control() || matches!(ch, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
}

pub fn resolve(app: &App, path: &str) -> Option<String> {
    let Route::Session(id) = app.navigation.current() else {
        return None;
    };
    let cwd = match &app.sessions.detail {
        Detail::Ready(item) if item.id == id => Some(item.workspace.host_cwd.as_str()),
        _ => None,
    };
    absolute(path, cwd)
}

fn absolute(path: &str, cwd: Option<&str>) -> Option<String> {
    if !valid_path(path) {
        return None;
    }
    if Path::new(path).is_absolute() {
        return Some(path.into());
    }
    let cwd = cwd.filter(|cwd| valid_path(cwd) && Path::new(cwd).is_absolute())?;
    // Preserve `..`: lexical normalization changes meaning across symlinks.
    let joined = Path::new(cwd).join(path).to_str()?.to_owned();
    valid_path(&joined).then_some(joined)
}

/// Freeze the Host-resolved path into each click target; never use the TUI's cwd.
pub fn resolve_hits(app: &mut App) {
    let mut hits = std::mem::take(&mut app.hits);
    hits.retain_mut(|hit| {
        if let Action::CopyFile(path) = &mut hit.action {
            if let Some(full) = resolve(app, path) {
                *path = full;
            } else {
                return false;
            }
        }
        true
    });
    app.hits = hits;
}

/// Color only the filename, preserving source bytes and unrelated span styles.
pub fn paint(line: &mut Line<'static>, bytes: Range<usize>, colors: Palette) -> Range<usize> {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let (Some(before), Some(link)) = (text.get(..bytes.start), text.get(bytes.clone())) else {
        return 0..0;
    };
    let columns = before.width()..before.width() + link.width();
    let mut offset = 0;
    for span in std::mem::take(&mut line.spans) {
        let start = bytes.start.saturating_sub(offset).min(span.content.len());
        let end = bytes.end.saturating_sub(offset).min(span.content.len());
        offset += span.content.len();
        if start >= end {
            line.spans.push(span);
            continue;
        }
        for (range, style) in [
            (0..start, span.style),
            (start..end, span.style.fg(colors.accent)),
            (end..span.content.len(), span.style),
        ] {
            if !range.is_empty() {
                line.spans
                    .push(Span::styled(span.content[range].to_owned(), style));
            }
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Modifier, Style};

    #[test]
    fn paths_are_literal_and_workspace_bound_while_links_preserve_width_and_style() {
        let cwd = std::env::temp_dir().join("maka-link-root");
        let cwd = cwd.to_str().unwrap();
        let name = "目录/it's $(literal); 🦀.rs";
        assert_eq!(
            absolute(name, Some(cwd)),
            Some(Path::new(cwd).join(name).to_str().unwrap().into())
        );
        assert_eq!(absolute(name, None), None);
        assert_eq!(absolute(name, Some("relative")), None);
        assert_eq!(absolute(cwd, None).as_deref(), Some(cwd));
        for invalid in [
            "",
            "  ",
            "https://example.com/a",
            "a\n.rs",
            "a\u{202e}rs",
            "a\x1b[31m",
        ] {
            assert_eq!(absolute(invalid, Some(cwd)), None);
        }
        let colors = Palette::default();
        let style = Style::default()
            .fg(colors.error)
            .add_modifier(Modifier::BOLD);
        let mut line = Line::from(vec![
            Span::styled("▾ Edit · ", style),
            Span::raw("目录/"),
            Span::raw("🦀.rs · failed"),
        ]);
        let original = line.to_string();
        let start = "▾ Edit · ".len();
        let end = start + "目录/🦀.rs".len();
        assert_eq!(paint(&mut line, start..end, colors), 9..19);
        assert_eq!(line.to_string(), original);
        assert_eq!(line.spans[0].style, style);
        assert_eq!(line.spans[1].style.fg, Some(colors.accent));
        assert_eq!(line.spans[2].style.fg, Some(colors.accent));
        assert_eq!(line.spans.last().unwrap().style, Style::default());
    }
}
