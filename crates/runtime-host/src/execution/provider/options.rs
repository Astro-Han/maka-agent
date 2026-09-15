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

use super::{Wire, unavailable};
use crate::provider_route::Route;
use maka_config::model_catalog::{ProviderFacts, ThinkingOffBehavior};
use maka_model::ProviderKind;
use maka_protocol::OperationError;
use maka_runtime::{configuration::ConnectionCatalogEntry, execution::ThinkingLevel};
use serde_json::{Value, json};
use std::borrow::Cow;

mod compatible;
mod relay;

/// Compose request options from the same admitted catalog row as routing.
/// SDKs encode the wire; this is Maka's explicit execution policy.
pub(super) fn resolve(
    row: &ConnectionCatalogEntry,
    facts: &ProviderFacts,
    model: &str,
    thinking_level: Option<ThinkingLevel>,
    route: &Route,
) -> Result<Value, OperationError> {
    let wire = route.wire;
    let known = facts.models.get(model);
    let stored = row.models.iter().find(|item| item.id == model);
    let parallel = row
        .model_overrides
        .as_ref()
        .and_then(|values| values.get(model))
        .and_then(|value| value.capabilities.as_ref())
        .and_then(|value| value.parallel_tool_calls)
        .or_else(|| {
            stored
                .and_then(|item| item.capabilities)
                .and_then(|caps| caps.parallel_tool_calls)
        })
        .or_else(|| {
            known
                .and_then(|item| item.metadata.capabilities)
                .and_then(|caps| caps.parallel_tool_calls)
        });
    if row.provider_type == "anthropic" {
        return Ok(anthropic(facts, model, thinking_level));
    }
    if row.provider_type == "openai-responses-compatible" && wire == Wire::OpenaiResponses {
        return relay::responses(row, model, thinking_level, parallel.unwrap_or(true));
    }
    if let ProviderKind::OpenaiCompatible { name } = &route.kind {
        return compatible::chat(name, model, thinking_level, parallel);
    }
    if !matches!(row.provider_type.as_str(), "openai" | "openai-codex") {
        if thinking_level.is_some() || parallel == Some(false) {
            return Err(unavailable(
                "Thinking or parallel-call policy is not supported for this provider",
            ));
        }
        return Ok(json!({}));
    }
    Ok(openai(
        wire,
        thinking_level,
        default_medium(facts, model),
        parallel.unwrap_or(true),
    ))
}

fn default_medium(facts: &ProviderFacts, model: &str) -> bool {
    let family = model.rsplit('/').next().unwrap_or(model);
    facts.models.get(family).is_some_and(|item| {
        item.metadata
            .thinking_options
            .as_ref()
            .and_then(|thinking| thinking.efforts.as_ref())
            .is_some_and(|efforts| efforts.iter().any(|effort| effort == "medium"))
    })
}

fn openai(wire: Wire, level: Option<ThinkingLevel>, default_medium: bool, parallel: bool) -> Value {
    let level = level.or(default_medium.then_some(ThinkingLevel::Medium));
    let mut options = json!({"store": false, "parallelToolCalls": parallel});
    if let Some(level) = level {
        options["reasoningEffort"] = match level {
            ThinkingLevel::Off => json!("none"),
            level => json!(level),
        };
    }
    if wire == Wire::OpenaiResponses && default_medium && level != Some(ThinkingLevel::Off) {
        options["reasoningSummary"] = json!("auto");
    }
    json!({"openai": options})
}

fn anthropic(facts: &ProviderFacts, model: &str, level: Option<ThinkingLevel>) -> Value {
    let family = claude_family(model);
    let known = facts.models.get(model);
    let family_facts = facts.models.get(family.as_ref());
    let metadata = known.map(|item| &item.metadata);
    let family_metadata = family_facts.map(|item| &item.metadata);
    let thinking = metadata
        .and_then(|metadata| metadata.thinking_options.as_ref())
        .or_else(|| family_metadata.and_then(|metadata| metadata.thinking_options.as_ref()));
    let bare_legacy = matches!(family.as_ref(), "claude-opus-4" | "claude-sonnet-4");
    let visible = family.starts_with("claude-")
        && (thinking.is_some_and(|thinking| {
            thinking.toggle == Some(true)
                || thinking
                    .efforts
                    .as_ref()
                    .is_some_and(|efforts| !efforts.is_empty())
        }) || metadata
            .and_then(|metadata| metadata.capabilities)
            .and_then(|caps| caps.reasoning)
            == Some(true)
            || family_metadata
                .and_then(|metadata| metadata.capabilities)
                .and_then(|caps| caps.reasoning)
                == Some(true)
            || bare_legacy);
    let mut options = json!({"cacheControl":{"type":"ephemeral"}});
    if level == Some(ThinkingLevel::Off)
        && thinking.and_then(|thinking| thinking.off_behavior)
            == Some(ThinkingOffBehavior::AnthropicThinkingDisabled)
    {
        options["thinking"] = json!({"type":"disabled"});
    } else if visible {
        let adaptive = !bare_legacy
            && known
                .or(family_facts)
                .is_some_and(|item| item.anthropic_adaptive_thinking == Some(true));
        options["thinking"] = if adaptive {
            json!({"type":"adaptive", "display":"summarized"})
        } else {
            json!({"type":"enabled", "budgetTokens":1024})
        };
        if let Some(level) = level {
            options["effort"] = json!(level);
        }
    } else if let Some(level) = level.filter(|level| *level != ThinkingLevel::Off) {
        options["effort"] = json!(level);
    }
    json!({"anthropic": options})
}

fn claude_family(model: &str) -> Cow<'_, str> {
    let family = model.rsplit('/').next().unwrap_or(model);
    for prefix in ["claude-haiku-", "claude-opus-", "claude-sonnet-"] {
        let Some((major, rest)) = family
            .strip_prefix(prefix)
            .and_then(|tail| tail.split_once('.'))
        else {
            continue;
        };
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if !major.is_empty()
            && major.bytes().all(|c| c.is_ascii_digit())
            && end > 0
            && (end == rest.len() || rest[end..].starts_with('-'))
        {
            return Cow::Owned(format!("{prefix}{major}-{rest}"));
        }
    }
    Cow::Borrowed(family)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_policy_uses_sdk_capabilities_without_enabling_unknown_models() {
        let facts = maka_config::model_catalog::provider_facts("anthropic").unwrap();
        let legacy = json!({"anthropic":{
            "cacheControl":{"type":"ephemeral"},
            "thinking":{"type":"enabled","budgetTokens":1024}
        }});
        for model in [
            "claude-sonnet-4-5",
            "relay/claude-sonnet-4.5",
            "claude-opus-4",
        ] {
            assert_eq!(anthropic(facts, model, None), legacy, "{model}");
        }
        assert_eq!(
            anthropic(facts, "claude-sonnet-4-5", Some(ThinkingLevel::Off)),
            json!({"anthropic":{
                "cacheControl":{"type":"ephemeral"}, "thinking":{"type":"disabled"}
            }}),
        );
        assert_eq!(
            anthropic(facts, "claude-opus-4-6", Some(ThinkingLevel::High)),
            json!({"anthropic":{
                "cacheControl":{"type":"ephemeral"},
                "thinking":{"type":"adaptive","display":"summarized"}, "effort":"high"
            }}),
        );
        for model in ["unknown", "claude-unknown", "claude-sonnet-4.5invalid"] {
            assert_eq!(
                anthropic(facts, model, None),
                json!({"anthropic":{
                    "cacheControl":{"type":"ephemeral"}
                }}),
                "{model}"
            );
        }
    }

    #[test]
    fn native_policy_distinguishes_default_off_and_unknown_models() {
        let facts = maka_config::model_catalog::provider_facts("openai").unwrap();
        let metadata = &facts.models["gpt-5.2"].metadata;
        assert!(
            metadata
                .thinking_options
                .as_ref()
                .unwrap()
                .efforts
                .as_ref()
                .unwrap()
                .iter()
                .any(|effort| effort == "medium")
        );
        assert_eq!(
            openai(Wire::OpenaiResponses, None, true, true),
            json!({"openai":{"store":false,"parallelToolCalls":true,"reasoningEffort":"medium","reasoningSummary":"auto"}}),
        );
        assert_eq!(
            openai(Wire::OpenaiResponses, Some(ThinkingLevel::Off), true, false),
            json!({"openai":{"store":false,"parallelToolCalls":false,"reasoningEffort":"none"}}),
        );
        assert_eq!(
            openai(Wire::OpenaiChat, Some(ThinkingLevel::High), true, false),
            json!({"openai":{"store":false,"parallelToolCalls":false,"reasoningEffort":"high"}}),
        );
        assert_eq!(
            openai(Wire::OpenaiResponses, None, false, true),
            json!({"openai":{"store":false,"parallelToolCalls":true}}),
        );
    }
}
