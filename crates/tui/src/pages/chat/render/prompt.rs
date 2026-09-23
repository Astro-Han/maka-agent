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

use super::*;
use unicode_width::UnicodeWidthStr;

/// Three-line prompt previews, following grok-build's user-message hierarchy.
pub(super) fn preview(text: &str, width: u16, ascii: bool) -> Result<(Layout, bool), &'static str> {
    // Enough for four wrapped lines, without laying out an entire pasted document.
    let prefix: String = text
        .graphemes(true)
        .take(usize::from(width) * 4 + 32)
        .collect();
    let mut layout = layout::plain(&prefix, width)?;
    let expandable = layout
        .lines
        .iter()
        .skip(3)
        .any(|line| !line.line.to_string().trim().is_empty())
        || !text[prefix.len()..].trim().is_empty();
    if !expandable {
        layout.lines.truncate(3);
        return Ok((layout, false));
    }
    layout.lines.truncate(3);
    let last = layout.lines.last_mut().unwrap();
    let ellipsis = if ascii { "." } else { "…" };
    let limit = usize::from(width.saturating_sub(1));
    let rendered = last.line.to_string();
    let mut cells = 0;
    let visible: String = rendered
        .graphemes(true)
        .take_while(|glyph| {
            cells += glyph.width();
            cells <= limit
        })
        .collect();
    let cut = visible.len();
    last.mapping.retain_mut(|span| {
        if span.display.start >= cut {
            return false;
        }
        if span.display.end > cut {
            // A partial expanded tab isn't a selectable source character.
            if !span.exact {
                return false;
            }
            let removed = span.display.end - cut;
            span.display.end = cut;
            span.logical.end -= removed;
            span.source.end -= removed;
        }
        true
    });
    last.line = Line::raw(format!("{visible}{ellipsis}"));
    let end = layout
        .lines
        .iter()
        .flat_map(|line| &line.mapping)
        .map(|span| span.logical.end)
        .max()
        .unwrap_or(0);
    layout.text.truncate(end);
    Ok((layout, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_prompts_stay_whole_long_prompts_preview_three_lines_without_copying_ellipsis_or_hidden_text()
     {
        for text in ["one", "one\ntwo", "one\ntwo\nthree"] {
            let (layout, expandable) = preview(text, 20, false).unwrap();
            assert!(!expandable);
            assert_eq!(layout.text, text);
        }
        let (layout, expandable) = preview("一二三四五六七八九十 hidden", 6, false).unwrap();
        assert!(expandable);
        assert_eq!(layout.lines.len(), 3);
        assert!(layout.lines[2].line.to_string().ends_with('…'));
        assert_eq!(layout.text, "一二三四五六七八");
        assert!(layout.lines.iter().all(|line| line.line.width() <= 6));
        let (wide, expandable) = preview("一二三四五六七八九十 hidden", 80, false).unwrap();
        assert!(!expandable);
        assert!(wide.text.ends_with("hidden"));
        assert!(
            !preview("one\n\n\n\n\n", 20, false).unwrap().1,
            "blank omitted rows do not offer an empty expansion"
        );
    }
}
