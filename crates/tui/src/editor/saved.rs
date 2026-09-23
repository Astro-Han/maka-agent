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

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub text: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub upstream: bool,
}

impl Saved {
    pub fn validate(&self) -> Result<(), &'static str> {
        let boundary = |offset| {
            offset == self.text.len()
                || self
                    .text
                    .grapheme_indices(true)
                    .any(|(start, _)| start == offset)
        };
        if self.text.len() > MAX_TEXT_BYTES
            || !boundary(self.cursor)
            || self.anchor.is_some_and(|offset| !boundary(offset))
            || self
                .text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            return Err("Invalid saved draft");
        }
        Ok(())
    }
}

impl Editor {
    pub fn save(&self) -> Saved {
        Saved {
            text: self.text.clone(),
            cursor: self.selection.cursor,
            anchor: self.selection.anchor,
            upstream: self.selection.upstream,
        }
    }
    pub fn restore(saved: Saved) -> Result<Self, &'static str> {
        saved.validate()?;
        let mut editor = Self {
            text: saved.text,
            selection: Selection {
                cursor: saved.cursor,
                anchor: saved.anchor,
                upstream: saved.upstream,
            },
            ..Self::default()
        };
        editor.reflow();
        Ok(editor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_draft_keeps_grapheme_selection_but_not_geometry_or_undo_and_rejects_bad_offsets() {
        let mut editor = Editor::default();
        editor.insert("中文🦀e\u{301}");
        editor.key(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT));
        let saved = editor.save();
        let restored = Editor::restore(saved).unwrap();
        assert_eq!(restored.text(), editor.text());
        assert_eq!(restored.selection.range(), editor.selection.range());
        assert!(restored.undo.is_empty() && restored.area.is_none());
        let mut invalid = editor.save();
        invalid.cursor -= 1;
        assert!(Editor::restore(invalid).is_err());
        let mut invalid = editor.save();
        invalid.text = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(Editor::restore(invalid).is_err());
    }
}
