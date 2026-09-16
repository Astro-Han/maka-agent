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
    Failure, copy_number, js_truthy, js_whitespace, object_array, optional_array, token_limit,
};
use serde_json::{Map, Value, json};

pub(super) fn codex(root: &Value) -> Result<Vec<Value>, Failure> {
    // Subscription inventories require their envelope; a missing field is not an empty list.
    let rows = root.get("models").ok_or(Failure::InvalidResponse)?;
    let mut rows: Vec<_> = object_array(Some(rows))?
        .iter()
        .filter(|row| {
            row["slug"]
                .as_str()
                .is_some_and(|id| !id.trim_matches(js_whitespace).is_empty())
                && !row["visibility"].as_str().is_some_and(|s| {
                    matches!(
                        s.trim_matches(js_whitespace).to_ascii_lowercase().as_str(),
                        "hide" | "hidden"
                    )
                })
        })
        .collect();
    let priority = |row: &Value| row["priority"].as_f64().unwrap_or(10_000.0);
    rows.sort_by(|a, b| {
        priority(a)
            .partial_cmp(&priority(b))
            .expect("finite JSON numbers")
    });
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut model = Map::new();
            model.insert("id".into(), row["slug"].clone());
            if row["context_window"].as_f64().is_some_and(|n| n > 0.0) {
                copy_number(row.get("context_window"), &mut model, "contextWindow");
            }
            Value::Object(model)
        })
        .collect())
}

pub(super) fn copilot(root: &Value) -> Result<Vec<Value>, Failure> {
    let rows = object_array(Some(root.get("data").ok_or(Failure::InvalidResponse)?))?;
    let mut models = Vec::new();
    for row in rows {
        if !eligible(row) {
            continue;
        }
        let endpoints = optional_array(row.get("supported_endpoints"))?;
        let supports = &row["capabilities"]["supports"];
        let limits = &row["capabilities"]["limits"];
        let efforts = optional_array(supports.get("reasoning_effort"))?;
        let media = optional_array(limits.pointer("/vision/supported_media_types"))?;
        let Some(protocol) = [
            ("/v1/messages", "anthropic-messages"),
            ("/responses", "openai-responses"),
            ("/chat/completions", "openai-chat"),
        ]
        .into_iter()
        .find_map(|(path, protocol)| {
            endpoints
                .iter()
                .any(|endpoint| endpoint == path)
                .then_some(protocol)
        }) else {
            continue;
        };
        let reasoning = supports["adaptive_thinking"] == true
            || !efforts.is_empty()
            || supports.get("max_thinking_budget").is_some()
            || supports.get("min_thinking_budget").is_some();
        let vision = supports["vision"] == true
            || media
                .iter()
                .any(|kind| kind.as_str().is_some_and(|kind| kind.starts_with("image/")));
        let mut model = Map::new();
        model.insert("id".into(), row["id"].clone());
        if let Some(name) = row.get("name").filter(|name| js_truthy(name)) {
            model.insert("displayName".into(), name.clone());
        }
        copy_number(
            limits
                .get("max_context_window_tokens")
                .filter(|n| token_limit(n).is_some())
                .or_else(|| limits.get("max_prompt_tokens")),
            &mut model,
            "contextWindow",
        );
        copy_number(
            limits.get("max_output_tokens"),
            &mut model,
            "maxOutputTokens",
        );
        model.insert("apiProtocol".into(), protocol.into());
        model.insert(
            "capabilities".into(),
            json!({"vision":vision,"reasoning":reasoning,"functionCalling":true}),
        );
        models.push(Value::Object(model));
    }
    // A known policy gate is refusal, not a malformed inventory or a usable model.
    if models.is_empty() && rows.iter().any(blocked_by_policy) {
        return Err(Failure::Auth);
    }
    Ok(models)
}

fn eligible(row: &Value) -> bool {
    row["id"].as_str().is_some_and(|id| !id.is_empty())
        && row["model_picker_enabled"] == true
        && row["capabilities"]["supports"]["tool_calls"] == true
        && row
            .get("policy")
            .is_none_or(|policy| policy.is_object() && policy["state"] == "enabled")
}

fn blocked_by_policy(row: &Value) -> bool {
    row["id"].as_str().is_some_and(|id| !id.is_empty())
        && row["model_picker_enabled"] == true
        && row["capabilities"]["supports"]["tool_calls"] == true
        && row["supported_endpoints"].as_array().is_some_and(|paths| {
            paths.iter().any(|path| {
                matches!(
                    path.as_str(),
                    Some("/v1/messages" | "/responses" | "/chat/completions")
                )
            })
        })
        && row["policy"].is_object()
        && matches!(
            row["policy"]["state"].as_str(),
            Some("disabled" | "unconfigured")
        )
}
