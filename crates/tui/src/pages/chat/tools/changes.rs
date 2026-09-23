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

//! Edit request evidence and bounded Host-captured Write previews.
//! No client-local filesystem reads or inferred historical snapshots.
use super::*;
use crate::pages::chat::layout::diff::{Kind, Row};
use similar::{ChangeTag, TextDiff};
use std::time::Duration;

const MAX_BYTES: usize = 8 * 1024;
const MAX_LINES: usize = 256;
pub(super) mod patch;

pub(super) fn result(name: &str, content: &Value) -> Option<String> {
    if content["kind"] != "json" {
        return None;
    }
    let value = &content["value"];
    let fields = value.as_object()?;
    value["path"].as_str()?;
    match name {
        "Edit" if fields.len() == 6 && value["ok"] == true && value["replacements"] == 1 => {
            let start = value["startLine"].as_u64()?;
            let end = value["endLine"].as_u64()?;
            if start == 0 || end < start {
                return None;
            }
            if !matches!(
                value["matchedVia"].as_str()?,
                "exact" | "line-trimmed" | "whitespace" | "escape"
            ) {
                return None;
            }
            Some(String::new()) // Ordinary success needs no repeated receipt.
        }
        "Write"
            if value["kind"] == "file_write"
                && (fields.len() == 3
                    || (fields.len() == 4
                        && value["previousContent"]
                            .as_str()
                            .is_some_and(|text| text.len() <= MAX_BYTES))) =>
        {
            value["bytes"].as_u64()?;
            Some(String::new())
        }
        _ => None,
    }
}

pub(super) fn append(
    call: &Value,
    completed: Option<&Value>,
    text: &mut String,
    i18n: &I18n,
) -> Option<Vec<Row>> {
    if !matches!(call["origin"].as_str(), Some("provider" | "code_mode")) {
        return None;
    }
    if call["toolName"] == "apply_patch" {
        return patch::append(&call["args"], text, i18n);
    }
    let args = call["args"].as_object()?;
    let path = args.get("path")?.as_str()?;
    if path.is_empty() {
        return None;
    }
    let (before, after) = match call["toolName"].as_str()? {
        "Edit" if args.len() == 3 => {
            let before = args.get("old_string")?.as_str()?;
            let after = args.get("new_string")?.as_str()?;
            if before.is_empty() || before == after {
                return None;
            }
            (Some(before), after)
        }
        "Write" if args.len() == 2 => {
            let after = args.get("content")?.as_str()?;
            let before = completed
                .filter(|result| result["isError"] == false)
                .map(|result| &result["content"])
                .filter(|content| {
                    result("Write", content).is_some()
                        && content["value"]["bytes"].as_u64() == Some(after.len() as u64)
                })
                .and_then(|content| content["value"]["previousContent"].as_str());
            (before, after)
        }
        _ => return None,
    };
    let old = before.unwrap_or("");
    if old.len() + after.len() > MAX_BYTES
        || old.lines().count() + after.lines().count() > MAX_LINES
        || old.contains('\0')
        || after.contains('\0')
    {
        return None;
    }
    let mut rows = Vec::new();
    if let Some(before) = before {
        // Small bounded inputs; the algorithm may use a coarser valid diff at
        // its deadline. Projection is cached by immutable transcript revision.
        let diff = TextDiff::configure()
            .timeout(Duration::from_millis(2))
            .diff_lines(before, after);
        for change in diff.iter_all_changes() {
            let kind = match change.tag() {
                ChangeTag::Delete => Kind::Removed,
                ChangeTag::Insert => Kind::Added,
                ChangeTag::Equal => Kind::Context,
            };
            append_line(text, &mut rows, change.value(), kind);
        }
    } else {
        for line in after.split_inclusive('\n') {
            append_line(text, &mut rows, line, Kind::Content);
        }
        if after.is_empty() {
            text.push_str(&format!("{}\n", i18n.text("tool-write-empty")));
        }
    }
    let language = crate::pages::chat::layout::syntax::language_for_path(path);
    for row in &mut rows {
        row.language = language;
    }
    Some(rows)
}

fn append_line(text: &mut String, rows: &mut Vec<Row>, value: &str, kind: Kind) {
    let start = text.len();
    text.push_str(value);
    if !value.ends_with('\n') {
        text.push('\n');
    }
    rows.push(Row {
        source: start..text.len(),
        kind,
        language: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Locale, LocalePreference};
    use serde_json::json;

    #[test]
    fn edit_write_and_patch_share_syntax_without_leaking_old_side_or_file_state_into_copy() {
        use crate::pages::chat::layout::{self, diff};
        let i18n = I18n::new(LocalePreference::Explicit(Locale::En), Locale::En);
        let code = "let name = \"中文🦀é\";\n";
        for call in [
            json!({"toolName":"Edit","origin":"code_mode","args":{"path":"C:\\remote\\main.rs","old_string":"/* removed\n","new_string":code}}),
            json!({"toolName":"Write","origin":"provider","args":{"path":"remote/main.rs","content":code}}),
            json!({"toolName":"apply_patch","origin":"code_mode","args":{"callId":"p","operation":{"type":"update_file","path":"remote/main.rs","diff":format!("@@\n-/* removed\n+{code}")}}}),
            json!({"toolName":"apply_patch","origin":"code_mode","args":format!("*** Begin Patch\n*** Update File: first.rs\n@@\n-/* removed\n+{code}@@\n let other = 42;\n*** Add File: second.py\n+def greet():\n+    return 42\n*** End Patch")}),
        ] {
            let mut text = String::new();
            let rows = append(&call, None, &mut text, &i18n).unwrap();
            assert!(rows.iter().any(|row| row.language == Some("Rust")));
            for choice in [
                crate::theme::Choice::Maka,
                crate::theme::Choice::Dusk,
                crate::theme::Choice::Paper,
                crate::theme::Choice::Terminal,
            ] {
                let colors = choice.colors();
                for width in [4, 80] {
                    let rendered =
                        diff::render_colored(&text, &rows, width, false, colors).unwrap();
                    assert_eq!(rendered.text, layout::plain(&text, width).unwrap().text);
                    for line in &rendered.lines {
                        let display = line.line.to_string();
                        assert!(line.line.width() <= usize::from(width));
                        for span in &line.mapping {
                            if span.exact {
                                assert_eq!(
                                    &display[span.display.clone()],
                                    &text[span.source.clone()]
                                );
                            }
                        }
                    }
                    if width == 80 {
                        let line = rendered
                            .lines
                            .iter()
                            .find(|line| line.line.to_string().contains("let name"))
                            .unwrap();
                        assert!(
                            line.line.spans.iter().any(|span| span.content == "let"
                                && span.style.fg == Some(colors.syntax[0]))
                        );
                        assert!(
                            line.line
                                .spans
                                .iter()
                                .any(|span| span.content.contains("中文🦀é")
                                    && span.style.fg == Some(colors.syntax[1]))
                        );
                        if rows.iter().any(|row| row.kind == Kind::Added) {
                            assert!(
                                line.line
                                    .spans
                                    .iter()
                                    .any(|span| span.content.starts_with("+ ")
                                        && span.style.fg == Some(colors.success))
                            );
                        }
                        if call["args"].is_string() {
                            let other = rendered
                                .lines
                                .iter()
                                .find(|line| line.line.to_string().contains("let other"))
                                .unwrap();
                            assert!(other.line.spans.iter().any(|span| span.content == "let"
                                && span.style.fg == Some(colors.syntax[0])));
                            let python = rendered
                                .lines
                                .iter()
                                .find(|line| line.line.to_string().contains("def greet"))
                                .unwrap();
                            assert!(python.line.spans.iter().any(|span| span.content == "def"
                                && span.style.fg == Some(colors.syntax[0])));
                        }
                    }
                }
            }
        }
        assert!(layout::syntax::language_for_path("remote/unknown.maka-unknown").is_none());
    }

    #[test]
    fn previews_are_request_evidence_even_after_fuzzy_success_and_keep_unknown_shapes_raw() {
        let call = json!({"toolName":"Edit","origin":"code_mode","args":{
            "path":"remote.rs","old_string":"same\nold 中文🦀\nend","new_string":"same\nnew é\nend"}});
        let result = json!({"isError":false,"content":{"kind":"json","value":{
            "ok":true,"path":"/remote/source.rs","replacements":1,"matchedVia":"whitespace","startLine":42,"endLine":44}}});
        for locale in Locale::ALL {
            let i18n = I18n::new(LocalePreference::Explicit(locale), locale);
            let mut card = Card {
                turn: "t",
                id: "edit",
                call: Some((1, &call)),
                result: None,
                closed: false,
            };
            for state in [
                State::Pending,
                State::Returned,
                State::Attention,
                State::Missing,
            ] {
                card.result = (state != State::Pending).then_some((2, &result));
                let projected = card.text(state, &i18n, false);
                assert!(projected.text.contains("old 中文🦀"));
                assert!(!projected.text.contains("old_string:"));
                assert!(
                    projected
                        .changes
                        .iter()
                        .any(|row| row.kind == Kind::Removed)
                );
                assert!(projected.changes.iter().any(|row| row.kind == Kind::Added));
                if state == State::Returned {
                    assert!(!projected.text.contains("whitespace"));
                    assert!(!projected.text.contains("42–44"));
                    assert!(!projected.text.contains("matchedVia"));
                } else if card.result.is_some() {
                    assert!(
                        projected.text.contains("whitespace"),
                        "result evidence is not replaced by a success claim"
                    );
                }
                let trace = card.text(state, &i18n, true);
                assert!(trace.changes.is_empty() && trace.text.contains("old_string:"));
                if card.result.is_some() {
                    assert!(trace.text.contains("whitespace"));
                }
            }
            assert!(i18n.diagnostics().is_empty());
        }
        let i18n = I18n::new(LocalePreference::Explicit(Locale::En), Locale::En);
        for (field, value) in [
            ("extra", json!("evidence")),
            ("matchedVia", json!("future")),
            ("endLine", json!(1)),
        ] {
            let mut content = result["content"].clone();
            content["value"][field] = value;
            assert!(super::result("Edit", &content).is_none());
        }
        for (field, value) in [
            ("metadata", json!({"keep":"evidence"})),
            ("old_string", json!("x".repeat(MAX_BYTES + 1))),
            ("old_string", json!("x\n".repeat(MAX_LINES + 1))),
            ("old_string", json!("binary\0data")),
            ("new_string", json!(null)),
        ] {
            let mut extended = call.clone();
            extended["args"][field] = value;
            let mut text = String::from("unchanged");
            assert!(append(&extended, None, &mut text, &i18n).is_none());
            assert_eq!(
                text, "unchanged",
                "fallback must not partially emit a preview"
            );
        }
        let write = json!({"toolName":"Write","origin":"provider","args":{"path":"existing.txt","content":"+literal\n"}});
        let mut text = String::new();
        let rows = append(&write, None, &mut text, &i18n).unwrap();
        assert!(rows.iter().all(|row| row.kind == Kind::Content));
        assert!(!text.contains("previous content unavailable"));
        for (before, failed) in [
            ("old content\n", false),
            ("", false),
            ("old content\n", true),
        ] {
            let completed = json!({"isError":failed,"content":{"kind":"json","value":{"kind":"file_write","path":"/workspace/existing.txt","bytes":9,"previousContent":before}}});
            let mut text = String::new();
            let rows = append(&write, Some(&completed), &mut text, &i18n).unwrap();
            assert_eq!(rows.iter().any(|row| row.kind == Kind::Added), !failed);
            assert_eq!(
                rows.iter().any(|row| row.kind == Kind::Removed),
                !failed && !before.is_empty()
            );
            assert_eq!(text.contains("old content"), !failed && !before.is_empty());
        }
        let mut empty = write;
        empty["args"]["content"] = json!("");
        assert!(append(&empty, None, &mut text, &i18n).unwrap().is_empty());
        assert!(text.contains("Empty content"));
    }
}
