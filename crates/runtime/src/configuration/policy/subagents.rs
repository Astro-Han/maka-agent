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

use crate::{configuration::present, execution::ThinkingLevel};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentProfile {
    LocalRead,
    WebResearch,
    Implementation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubagentPreset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub profile: SubagentProfile,
    pub connection_slug: String,
    pub model: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub thinking_level: Option<ThinkingLevel>,
    pub enabled: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentSettings {
    pub presets: Vec<SubagentPreset>,
}

pub(super) fn normalize(input: &Value) -> SubagentSettings {
    let mut presets = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(values) = input.get("presets").and_then(Value::as_array) {
        for value in values {
            if presets.len() >= 64 {
                break;
            }
            let Some(preset) = candidate(value) else {
                continue;
            };
            if seen.insert(preset.id.clone()) {
                presets.push(preset);
            }
        }
    }
    SubagentSettings { presets }
}

fn candidate(value: &Value) -> Option<SubagentPreset> {
    let id = value.get("id")?.as_str()?;
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
    {
        return None;
    }
    let required = |field: &str, max| {
        let text = super::codec::trim(value.get(field)?.as_str()?);
        (!text.is_empty() && text.encode_utf16().count() <= max).then(|| text.to_owned())
    };
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let description = scalar_prefix(super::codec::trim(description), 1_000);
    Some(SubagentPreset {
        id: id.into(),
        name: required("name", 128)?,
        description,
        profile: serde_json::from_value(value.get("profile")?.clone()).ok()?,
        connection_slug: required("connectionSlug", 128)?,
        model: required("model", 512)?,
        thinking_level: match value.get("thinkingLevel") {
            None => None,
            Some(value) => Some(serde_json::from_value(value.clone()).ok()?),
        },
        enabled: value.get("enabled")?.as_bool()?,
    })
}
fn scalar_prefix(value: &str, maximum: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= maximum
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn permissive_normalization_drops_invalid_presets_and_deduplicates_after_validation() {
        let valid = json!({"id":"a","name":"\u{feff} agent ","profile":"local_read",
            "connectionSlug":" any slug ","model":" model ","enabled":false,"extra":true});
        let result = normalize(&json!({"extra":true,"presets":[null,{"id":"a"},valid,valid]}));
        assert_eq!(result.presets.len(), 1);
        assert_eq!(result.presets[0].name, "agent");
        assert_eq!(result.presets[0].connection_slug, "any slug");
        assert!(result.presets[0].description.is_empty());
        assert!(normalize(&Value::Null).presets.is_empty());
    }
    #[test]
    fn description_boundary_preserves_scalar_instead_of_typescript_lone_surrogate() {
        let text = format!("{}😀", "a".repeat(999));
        let prefix = scalar_prefix(&text, 1000);
        // TS slice(0,1000) includes an unpaired high surrogate. This deliberate
        // safe deviation uses 999 units, not a replacement or WTF-16 string type.
        assert_eq!(prefix, "a".repeat(999));
        assert_eq!(scalar_prefix(&text, 1001), text);
    }
}
