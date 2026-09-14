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

use super::{ConnectionCatalogEntry, OperationError, ThinkingLevel, Value, Wire};
use maka_config::model_catalog::provider_facts;

pub(super) fn responses(
    row: &ConnectionCatalogEntry,
    model: &str,
    level: Option<ThinkingLevel>,
    parallel: bool,
) -> Result<Value, OperationError> {
    // Relay catalogs may have no models. Defaults and summaries follow canonical
    // OpenAI family facts, never the relay's declared thinking levels.
    let facts = provider_facts("openai")
        .map_err(|_| super::unavailable("OpenAI provider facts are unavailable"))?;
    let mut options = super::openai(
        Wire::OpenaiResponses,
        level,
        super::default_medium(facts, model),
        parallel,
    );
    options["openai"]["forceReasoning"] = Value::Bool(true);
    if supports_fast(model)
        && let Some(tier) = row
            .model_overrides
            .as_ref()
            .and_then(|profiles| profiles.get(model))
            .and_then(|profile| profile.service_tier.as_ref())
    {
        options["openai"]["serviceTier"] = serde_json::json!(tier);
    }
    Ok(options)
}

// core/model-thinking.ts supportsRelayFastServiceTier deliberately tests the
// bare model ID: a relay/ prefix must not inherit priority processing support.
fn supports_fast(model: &str) -> bool {
    if model.starts_with("gpt-4") {
        return true;
    }
    if let Some(tail) = model.strip_prefix('o') {
        let version = tail.split('-').next().unwrap_or(tail);
        return version_at_least(version, '3');
    }
    let Some(tail) = model.strip_prefix("gpt-") else {
        return false;
    };
    let (version, variant) = tail
        .split_once('-')
        .map_or((tail, None), |(version, variant)| (version, Some(variant)));
    if variant.is_some_and(|value| {
        value.is_empty() || value.starts_with("nano") || value.starts_with("chat")
    }) {
        return false;
    }
    let (major, minor) = version
        .split_once('.')
        .map_or((version, None), |(major, minor)| (major, Some(minor)));
    version_at_least(major, '5')
        && minor.is_none_or(|value| !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()))
}

fn version_at_least(version: &str, minimum: char) -> bool {
    if version.is_empty() || !version.bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let significant = version.trim_start_matches('0');
    significant.len() > 1
        || significant
            .chars()
            .next()
            .is_some_and(|digit| digit >= minimum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn relay_policy_keeps_declared_effort_separate_from_family_defaults_and_fast_gate() {
        let row: ConnectionCatalogEntry = serde_json::from_value(json!({
            "connectionId":"relay", "revision":1, "slug":"relay", "name":"Relay",
            "providerType":"openai-responses-compatible", "enabled":true,
            "enabledModelIds":["unknown", "gpt-5.2", "relay/gpt-5.2"], "models":[],
            "modelOverrides":{
                "unknown":{"thinkingLevels":["max"],"serviceTier":"fast"},
                "gpt-5.2":{"serviceTier":"fast"},
                "relay/gpt-5.2":{"serviceTier":"fast"}
            }
        }))
        .unwrap();
        let unknown = json!({"openai":{
            "store":false,"parallelToolCalls":false,"forceReasoning":true
        }});
        assert_eq!(responses(&row, "unknown", None, false).unwrap(), unknown);
        let mut explicit = unknown;
        explicit["openai"]["reasoningEffort"] = json!("max");
        assert_eq!(
            responses(&row, "unknown", Some(ThinkingLevel::Max), false).unwrap(),
            explicit
        );
        for (model, fast) in [("gpt-5.2", true), ("relay/gpt-5.2", false)] {
            let result = responses(&row, model, None, true).unwrap();
            assert_eq!(result["openai"]["reasoningEffort"], "medium");
            assert_eq!(result["openai"]["reasoningSummary"], "auto");
            assert_eq!(result["openai"]["serviceTier"] == "fast", fast);
        }
        for model in [
            "gpt-4o",
            "gpt-4.1-nano",
            "gpt-5",
            "gpt-5.2-pro",
            "o3",
            "o4-mini",
        ] {
            assert!(supports_fast(model), "{model}");
        }
        for model in [
            "unknown",
            "gpt-5-nano",
            "gpt-5-chat-latest",
            "relay/gpt-5",
            "o2",
            "o3x",
            "gpt-5.",
            "gpt-5-",
        ] {
            assert!(!supports_fast(model), "{model}");
        }
    }
}
