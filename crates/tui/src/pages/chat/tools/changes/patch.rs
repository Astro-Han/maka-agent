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

//! Display the executor's grammar without gaining filesystem authority.
//! Validation uses the shared pure parser; request text is never a file snapshot.
use super::{Kind, MAX_BYTES, MAX_LINES, Row};
use crate::i18n::I18n;
use apply_patch::PatchOperation;
use serde_json::Value;

fn bounded(text: &str) -> bool {
    text.len() <= MAX_BYTES && text.lines().count() <= MAX_LINES && !text.contains('\0')
}

fn path_valid(path: &str) -> bool {
    !path.is_empty() && path.len() <= 4096 && !path.chars().any(char::is_control)
}

fn label(kind: &str) -> Option<&'static str> {
    match kind {
        "create_file" => Some("tool-patch-create"),
        "update_file" => Some("tool-patch-update"),
        "delete_file" => Some("tool-patch-delete"),
        _ => None,
    }
}

fn header(line: &str) -> Option<(&'static str, &str)> {
    for (prefix, kind) in [
        ("*** Add File: ", "create_file"),
        ("*** Update File: ", "update_file"),
        ("*** Delete File: ", "delete_file"),
    ] {
        if let Some(path) = line.strip_prefix(prefix) {
            return Some((kind, path.trim_end()));
        }
    }
    None
}

fn file(text: &mut String, kind: &str, path: &str, i18n: &I18n) {
    text.push_str(&format!("\n{} · {path}\n", i18n.text(label(kind).unwrap())));
}

fn line(text: &mut String, rows: &mut Vec<Row>, raw: &str, language: Option<&'static str>) {
    let (kind, body) = match raw.as_bytes().first() {
        Some(b'+') => (Kind::Added, &raw[1..]),
        Some(b'-') => (Kind::Removed, &raw[1..]),
        Some(b' ') => (Kind::Context, &raw[1..]),
        _ => {
            text.push_str(raw);
            text.push('\n');
            return;
        }
    };
    let start = text.len();
    text.push_str(body);
    text.push('\n');
    rows.push(Row {
        source: start..text.len(),
        kind,
        language,
    });
}

pub(super) fn append(args: &Value, text: &mut String, i18n: &I18n) -> Option<Vec<Row>> {
    let mut rows = Vec::new();
    if let Some(patch) = args.as_str() {
        if !bounded(patch) {
            return None;
        }
        let operations = apply_patch::parse_patch(patch).ok()?;
        let lines: Vec<_> = patch.trim().lines().collect();
        let body = &lines[1..lines.len() - 1];
        // Prefer raw fallback for padded operation headers. A context line
        // containing " *** Add File:" must never become a synthetic file.
        let headers: Vec<_> = body.iter().filter_map(|line| header(line)).collect();
        if headers.len() != operations.len()
            || !headers
                .iter()
                .zip(&operations)
                .all(|((kind, path), operation)| {
                    let (expected_kind, expected_path) = match operation {
                        PatchOperation::Add { path, .. } => ("create_file", path),
                        PatchOperation::Update { path, .. } => ("update_file", path),
                        PatchOperation::Delete { path } => ("delete_file", path),
                    };
                    *kind == expected_kind
                        && path_valid(path)
                        && expected_path.to_str() == Some(*path)
                })
        {
            return None;
        }
        let mut language = None;
        for raw in body {
            if let Some((kind, path)) = header(raw) {
                file(text, kind, path, i18n);
                language = crate::pages::chat::layout::syntax::language_for_path(path);
            } else {
                line(text, &mut rows, raw, language);
            }
        }
    } else {
        let object = args.as_object()?;
        let id = object.get("callId")?.as_str()?;
        let operation = &args["operation"];
        let fields = operation.as_object()?;
        let kind = operation["type"].as_str()?;
        label(kind)?;
        let path = operation["path"].as_str()?;
        if object.len() != 2 || id.is_empty() || id.len() > 4096 || !path_valid(path) {
            return None;
        }
        let (diff, created) = if kind == "delete_file" {
            if fields.len() != 2 {
                return None;
            }
            ("", None)
        } else {
            if fields.len() != 3 {
                return None;
            }
            let diff = operation["diff"].as_str()?;
            if !bounded(diff) {
                return None;
            }
            let created = match kind {
                "create_file" => Some(apply_patch::parse_create(diff).ok()?),
                "update_file" => {
                    apply_patch::parse_update(diff).ok()?;
                    None
                }
                _ => return None,
            };
            (diff, created)
        };
        if kind == "delete_file" {
            text.push_str(&format!("{}\n", i18n.text("tool-patch-delete")));
        }
        let language = crate::pages::chat::layout::syntax::language_for_path(path);
        if let Some(created) = created {
            if created.is_empty() {
                text.push_str(&format!("{}\n", i18n.text("tool-write-empty")));
            }
            for line in created.split_inclusive('\n') {
                super::append_line(text, &mut rows, line, Kind::Added);
            }
            for row in &mut rows {
                row.language = language;
            }
        } else {
            for raw in diff.lines() {
                line(text, &mut rows, raw, language);
            }
        }
    }
    Some(rows)
}

/// A batch failure is an application result, not necessarily an outer tool error.
/// Unknown extensions still get attention; they remain raw in result().
pub fn failed(content: &Value) -> bool {
    content["kind"] == "json" && content["value"]["status"] == "failed"
}

fn operation(value: &Value, i18n: &I18n) -> Option<String> {
    if value.as_object()?.len() != 2 {
        return None;
    }
    let name = label(value["type"].as_str()?)?;
    let path = value["path"].as_str()?;
    if !path_valid(path) {
        return None;
    }
    Some(format!("{} · {path}", i18n.text(name)))
}

pub fn result(content: &Value, i18n: &I18n) -> Option<String> {
    if content["kind"] != "json" {
        return None;
    }
    let value = &content["value"];
    let fields = value.as_object()?;
    let status = value["status"].as_str()?;
    if fields.len() == 1 && status == "completed" {
        return Some(String::new());
    }
    let applied = value["applied"].as_array()?;
    if applied.len() > 128 {
        return None;
    }
    let operations = applied
        .iter()
        .map(|value| operation(value, i18n))
        .collect::<Option<Vec<_>>>()?;
    let count = applied.len().to_string();
    let (summary, boundary) = match status {
        "completed" if fields.len() == 3 => {
            // Do not discard future/custom output under a familiar shape.
            if value["output"]
                != format!(
                    "Applied {} file operation{}.",
                    applied.len(),
                    if applied.len() == 1 { "" } else { "s" }
                )
            {
                return None;
            }
            return Some(String::new());
        }
        "failed" if fields.len() == 4 => {
            let (key, title, state) = if fields.contains_key("failed") {
                ("failed", "tool-patch-failed-at", "tool-patch-failed")
            } else {
                (
                    "stoppedBefore",
                    "tool-patch-stopped-before",
                    "tool-patch-stopped",
                )
            };
            operation(&value[key], i18n)?;
            let error = value["error"].as_str()?;
            if !bounded(error) {
                return None;
            }
            (
                i18n.format(state, &[("count", &count)]),
                format!(
                    "\n{}\n{error}",
                    i18n.format(title, &[("path", value[key]["path"].as_str()?)])
                ),
            )
        }
        _ => return None,
    };
    let mut text = summary;
    for operation in operations {
        text.push_str(&format!("\n{operation}"));
    }
    text.push_str(&boundary);
    if text.len() > MAX_BYTES {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        i18n::{Locale, LocalePreference},
        pages::chat::{
            layout::diff,
            tools::{Card, State},
        },
    };
    use serde_json::json;

    #[test]
    fn shared_patch_grammar_preserves_file_order_literal_markers_and_delete_uncertainty() {
        let patch = "*** Begin Patch\n*** Add File: first\n++literal 中文🦀\n*** Update File: first\n@@\n-+literal 中文🦀\n+updated\n *** Add File: not-a-file\n*** End of File\n*** Delete File: later\n*** End Patch";
        for locale in Locale::ALL {
            let i18n = I18n::new(LocalePreference::Explicit(locale), locale);
            let call = json!({"toolName":"apply_patch","origin":"code_mode","args":patch});
            let card = Card {
                turn: "t",
                id: "p",
                call: Some((1, &call)),
                result: None,
                closed: false,
            };
            let projected = card.text(State::Pending, &i18n, false);
            assert!(!projected.changes.is_empty());
            assert!(projected.text.contains("+literal 中文🦀"));
            let mut text = String::new();
            let rows = append(&json!(patch), &mut text, &i18n).unwrap();
            assert!(!text.contains("not a file snapshot"));
            assert_eq!(
                text.matches("· first").count(),
                2,
                "repeated paths are sequential operations"
            );
            assert!(!text.contains("· not-a-file"));
            for width in [4, 20, 80] {
                let layout = diff::render(&text, &rows, width, false).unwrap();
                assert!(layout.text.contains("+literal 中文🦀\n"));
                assert!(!layout.text.contains("++literal"));
                assert!(layout.text.contains("*** Add File: not-a-file"));
            }
            for operation in [
                json!({"type":"create_file","path":"empty-line","diff":"+"}),
                json!({"type":"update_file","path":"x","diff":"@@\n-old\n+new"}),
                json!({"type":"delete_file","path":"x"}),
            ] {
                assert!(
                    append(
                        &json!({"callId":"opaque","operation":operation}),
                        &mut String::new(),
                        &i18n
                    )
                    .is_some()
                );
            }
            assert!(i18n.diagnostics().is_empty());
        }
        let i18n = I18n::new(LocalePreference::Explicit(Locale::En), Locale::En);
        for (diff, lines) in [("+", 0), ("+one\n+two\n+", 2)] {
            let input = json!({"callId":"create","operation":{"type":"create_file","path":"x","diff":diff}});
            let mut text = String::new();
            let rows = append(&input, &mut text, &i18n).unwrap();
            assert_eq!(
                rows.len(),
                lines,
                "native create must not invent an extra final blank line"
            );
            if lines == 0 {
                assert!(text.contains("Empty content"));
            }
        }
        for invalid in [
            json!("*** Begin Patch\n*** Update File: x\n*** Move to: y\n@@\n-x\n+y\n*** End Patch"),
            json!({"callId":"id","operation":{"type":"update_file","path":"x","diff":"@@\n-x\n+y\n*** Delete File: injected"}}),
            json!({"callId":"id","operation":{"type":"delete_file","path":"x","future":true}}),
            json!({"callId":"id","operation":{"type":"delete_file","path":"misleading\nheader"}}),
            json!("x".repeat(MAX_BYTES + 1)),
        ] {
            let mut text = String::from("retained");
            assert!(append(&invalid, &mut text, &i18n).is_none());
            assert_eq!(text, "retained");
        }
    }

    #[test]
    fn partial_batch_failure_is_attention_even_without_outer_error_and_never_hides_extensions() {
        let call = json!({"toolName":"apply_patch","origin":"code_mode","args":"*** Begin Patch\n*** Delete File: later\n*** End Patch"});
        let returned = json!({"isError":false,"content":{"kind":"json","value":{
            "status":"failed","applied":[{"type":"create_file","path":"created"}],
            "failed":{"type":"update_file","path":"existing"},"error":"context did not match"}}});
        for locale in Locale::ALL {
            let i18n = I18n::new(LocalePreference::Explicit(locale), locale);
            for stopped in [false, true] {
                let mut returned = returned.clone();
                if stopped {
                    let value = returned["content"]["value"].as_object_mut().unwrap();
                    let boundary = value.remove("failed").unwrap();
                    value.insert("stoppedBefore".into(), boundary);
                }
                let card = Card {
                    turn: "t",
                    id: "p",
                    call: Some((1, &call)),
                    result: Some((2, &returned)),
                    closed: true,
                };
                assert!(card.state(false) == State::Attention);
                let content = card.text(card.state(false), &i18n, false);
                assert!(content.text.lines().next().unwrap().contains(&i18n.format(
                    if stopped {
                        "tool-patch-stopped"
                    } else {
                        "tool-patch-failed"
                    },
                    &[("count", "1")]
                )));
                assert!(
                    content.text.contains("created")
                        && content.text.contains("existing")
                        && content.text.contains("context did not match")
                );
                assert!(!content.text.contains("\"applied\""));
                assert!(
                    card.text(card.state(false), &i18n, true)
                        .text
                        .contains("\"applied\"")
                );
            }
            assert!(i18n.diagnostics().is_empty());
        }
        let i18n = I18n::new(LocalePreference::Explicit(Locale::En), Locale::En);
        let mut unknown = returned["content"].clone();
        unknown["value"]["future"] = json!("must remain visible");
        assert!(failed(&unknown));
        assert!(result(&unknown, &i18n).is_none());
        let mut foreign = call.clone();
        foreign["toolName"] = json!("mcp__other__apply_patch");
        let card = Card {
            turn: "t",
            id: "p",
            call: Some((1, &foreign)),
            result: Some((2, &returned)),
            closed: true,
        };
        assert!(
            card.state(false) == State::Returned,
            "do not reinterpret a third-party result protocol"
        );
        let completed = json!({"kind":"json","value":{"status":"completed","applied":[{"type":"delete_file","path":"gone"}],"output":"Applied 1 file operation."}});
        assert!(result(&completed, &i18n).unwrap().is_empty());
        let mut extended = completed;
        extended["value"]["output"] = json!("custom evidence");
        assert!(result(&extended, &i18n).is_none());
        for (content, expected) in [
            (
                json!({"kind":"text","text":"short failure"}),
                "short failure".into(),
            ),
            (
                json!({"kind":"text","text":"long failure ".repeat(40)}),
                "long failure ".repeat(40),
            ),
            (
                json!({"kind":"text","text":"short failure","metadata":"keep this"}),
                "keep this".into(),
            ),
        ] {
            let returned = json!({"isError":true,"content":content});
            let card = Card {
                turn: "t",
                id: "p",
                call: Some((1, &call)),
                result: Some((2, &returned)),
                closed: true,
            };
            let projected = card.text(card.state(false), &i18n, false);
            assert!(projected.text.contains(&expected));
            if expected == "short failure" {
                assert_eq!(projected.text.matches(&expected).count(), 1);
            }
        }
    }
}
