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
use std::collections::HashSet;

use maka_runtime::configuration::{ConnectionEffectFailureClass, ModelInfo};
use serde_json::{Map, Value};

type Failure = ConnectionEffectFailureClass;
use super::DiscoveryKind;
mod subscription;

/// Decode the native providers' `data` envelope into canonical catalog rows.
pub(super) fn decode(body: &[u8], kind: DiscoveryKind) -> Result<Vec<ModelInfo>, Failure> {
    let text = String::from_utf8_lossy(body);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let root: Value = serde_json::from_str(text).map_err(|_| Failure::InvalidResponse)?;
    if !root.is_object() && !root.is_array() {
        return Err(Failure::InvalidResponse);
    }
    let rows = match kind {
        DiscoveryKind::Codex => subscription::codex(&root)?,
        DiscoveryKind::Copilot => subscription::copilot(&root)?,
        _ => object_array(root.get("data"))?
            .iter()
            .map(model_info)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect(),
    };
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for mut model in rows {
        let id = model["id"].as_str().unwrap().trim_matches(js_whitespace);
        if id.is_empty()
            || id.encode_utf16().count() > 512
            || id.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
            || !seen.insert(id.to_owned())
        {
            continue;
        }
        model["id"] = Value::String(id.to_owned());
        let model: ModelInfo =
            serde_json::from_value(model).map_err(|_| Failure::InvalidResponse)?;
        model.validate().map_err(|_| Failure::InvalidResponse)?;
        models.push(model);
        if models.len() > 2048 {
            return Err(Failure::InvalidResponse);
        }
    }
    Ok(models)
}

fn optional_array(value: Option<&Value>) -> Result<&[Value], Failure> {
    match value {
        None => Ok(&[]),
        Some(Value::Array(values)) => Ok(values),
        _ => Err(Failure::InvalidResponse),
    }
}

fn object_array(value: Option<&Value>) -> Result<&[Value], Failure> {
    let values = optional_array(value)?;
    if values.iter().any(|v| !v.is_object()) {
        return Err(Failure::InvalidResponse);
    }
    Ok(values)
}

fn contains(values: &[Value], expected: &str) -> bool {
    values.iter().any(|v| v.as_str() == Some(expected))
}

fn copy_bool(source: Option<&Value>, target: &mut Map<String, Value>, key: &str) {
    if let Some(Value::Bool(value)) = source {
        target.insert(key.into(), Value::Bool(*value));
    }
}

fn model_info(row: &Value) -> Result<Option<Value>, Failure> {
    let Some(id) = row
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    let input = optional_array(row.get("input_modalities"))?;
    let output = optional_array(row.get("output_modalities"))?;
    let tags = optional_array(row.get("tags"))?;
    let providers = object_array(row.get("providers"))?;
    let mut caps = Map::new();
    if contains(input, "image") {
        caps.insert("vision".into(), true.into());
    }
    copy_bool(
        row.get("capabilities").and_then(|v| v.get("reasoning")),
        &mut caps,
        "reasoning",
    );
    copy_bool(row.get("supports_image_in"), &mut caps, "vision");
    copy_bool(row.get("supports_reasoning"), &mut caps, "reasoning");
    for (tag, key) in [
        ("vision", "vision"),
        ("reasoning", "reasoning"),
        ("tool-use", "functionCalling"),
    ] {
        if contains(tags, tag) {
            caps.insert(key.into(), true.into());
        }
    }
    let known_output = ["text", "image", "audio", "pdf", "video"]
        .iter()
        .any(|kind| contains(output, kind));
    if known_output && !contains(output, "text") {
        caps.insert("chat".into(), false.into());
        if contains(output, "image") {
            caps.insert("imageGeneration".into(), true.into());
        }
    }
    // A present providers array (including []) overrides the tool-use tag.
    if row.get("providers").is_some() {
        caps.insert(
            "functionCalling".into(),
            Value::Bool(providers.iter().any(|provider| {
                provider.get("status").and_then(Value::as_str) == Some("live")
                    && provider.get("supports_tools").and_then(Value::as_bool) == Some(true)
            })),
        );
    }
    let mut model = Map::new();
    model.insert("id".into(), id.into());
    let display = row.get("display_name");
    let name = row.get("name");
    if display.is_some_and(js_truthy) || name.is_some_and(js_truthy) {
        // JS uses truthiness to include the field, then nullish coalescing to select it.
        if let Some(value) = display.filter(|v| !v.is_null()).or(name) {
            model.insert("displayName".into(), value.clone());
        }
    }
    let context = row
        .get("context_length")
        .filter(|v| token_limit(v).is_some())
        .or(row.get("context_window"));
    copy_number(context, &mut model, "contextWindow");
    copy_number(row.get("max_tokens"), &mut model, "maxOutputTokens");
    if !caps.is_empty() {
        model.insert("capabilities".into(), Value::Object(caps));
    }
    Ok(Some(Value::Object(model)))
}

fn token_limit(value: &Value) -> Option<u64> {
    value
        .as_f64()
        .filter(|n| (1.0..=9_007_199_254_740_991.0).contains(n) && n.fract() == 0.0)
        .map(|n| n as u64)
}

fn copy_number(value: Option<&Value>, model: &mut Map<String, Value>, key: &str) {
    if let Some(limit) = value.and_then(token_limit) {
        model.insert(key.into(), limit.into());
    }
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64() != Some(0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

pub(super) fn js_whitespace(c: char) -> bool {
    matches!(c, '\u{9}'..='\u{d}' | '\u{20}' | '\u{a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}'
        | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode(body: &[u8]) -> Result<Vec<Value>, Failure> {
        super::decode(body, DiscoveryKind::Openai).map(|models| {
            models
                .into_iter()
                .map(|model| serde_json::to_value(model).unwrap())
                .collect()
        })
    }

    #[test]
    fn metadata_precedence_and_unknown_fields() {
        let rows = decode(
            br#"{"data":[{"id":" x ","display_name":"","name":"fallback",
            "context_length":0,"context_window":4096.0,"max_tokens":1e3,
            "input_modalities":["image"],"output_modalities":[null,"Text","image"],
            "supports_image_in":false,"capabilities":{"reasoning":true},
            "supports_reasoning":false,"tags":["reasoning","tool-use"],"providers":[],
            "unknown":true}]}"#,
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![json!({"id":"x","displayName":"","contextWindow":4096,
            "maxOutputTokens":1000,"capabilities":{"vision":false,"reasoning":true,
            "functionCalling":false,"chat":false,"imageGeneration":true}})]
        );
    }

    #[test]
    fn first_duplicate_wins_and_ids_use_javascript_text_rules() {
        let body = json!({"data":[{"id":"\u{feff}x\u{feff}","name":"first"},
            {"id":"x","name":42},{"id":"a\u{7f}"},{"id":"😀".repeat(257)},
            {"id":"😀".repeat(256)},{"id":"\u{85}y\u{85}"}]})
        .to_string();
        let rows = decode(body.as_bytes()).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], json!({"id":"x","displayName":"first"}));
        assert_eq!(rows[2]["id"], "\u{85}y\u{85}");
    }

    #[test]
    fn envelope_and_metadata_structure_failures() {
        for body in [
            "null",
            "true",
            "{",
            r#"{"data":null}"#,
            r#"{"data":[1]}"#,
            r#"{"data":[{"id":"x","providers":[null]}]}"#,
            r#"{"data":[{"id":"x","tags":null}]}"#,
            r#"{"data":[{"id":"x","name":42}]}"#,
            r#"{"data":[{"id":"x"},{"id":"x","input_modalities":null}]}"#,
        ] {
            assert_eq!(
                decode(body.as_bytes()),
                Err(Failure::InvalidResponse),
                "{body}"
            );
        }
        for body in [
            "{}",
            "[]",
            r#"{"data":[{"name":"missing id","tags":null}]}"#,
        ] {
            assert!(decode(body.as_bytes()).unwrap().is_empty());
        }
    }

    #[test]
    fn model_count_is_bounded_after_deduplication() {
        let mut rows: Vec<Value> = (0..2048).map(|i| json!({"id":i.to_string()})).collect();
        rows.push(json!({"id":"0"}));
        assert_eq!(
            decode(json!({"data":rows}).to_string().as_bytes())
                .unwrap()
                .len(),
            2048
        );
        rows.push(json!({"id":"extra"}));
        assert_eq!(
            decode(json!({"data":rows}).to_string().as_bytes()),
            Err(Failure::InvalidResponse)
        );
    }

    #[test]
    fn response_text_uses_lossy_utf8_and_strips_bom() {
        assert_eq!(
            decode(b"\xef\xbb\xbf{\"data\":[{\"id\":\"\xff\"}]}").unwrap(),
            vec![json!({"id":"\u{fffd}"})]
        );
    }
}
