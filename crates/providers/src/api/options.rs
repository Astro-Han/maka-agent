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

use super::{Selection, route::Route};
use super::{Wire, unavailable};
use crate::facts::{ProviderFacts, ThinkingOffBehavior};
use maka_plugins::model::ProviderKind;
use maka_plugins::provider::Error;
use maka_runtime::execution::ThinkingLevel;
use serde_json::{Value, json};
use std::borrow::Cow;

mod compatible;
mod relay;

/// Compose protocol options from the same public request as routing.
/// SDKs encode the wire; this is Maka's explicit execution policy.
pub(super) fn resolve(
    row: &Selection<'_>,
    facts: &ProviderFacts,
    thinking_level: Option<ThinkingLevel>,
    route: &Route,
) -> Result<Value, Error> {
    let model = row.model.id.as_str();
    let wire = route.wire;
    let known = facts.models.get(model);
    let stored = Some(row.model);
    let parallel = row
        .overrides
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
    if let ProviderKind::OpenResponses(contract) = &route.kind {
        if parallel == Some(false) {
            return Err(unavailable(
                "Open Responses does not expose parallel-call policy",
            ));
        }
        let mut options = json!({});
        if let Some(level) = thinking_level {
            options["reasoningEffort"] = if level == ThinkingLevel::Off {
                json!("none")
            } else {
                json!(level)
            };
        }
        if contract.reasoning_replay
            == maka_runtime::model::PlaintextReasoningReplay::PlaintextSummary
            && thinking_level != Some(ThinkingLevel::Off)
        {
            options["reasoningSummary"] = json!("auto");
        }
        return Ok(json!({"openResponses":options}));
    }
    if row.provider == "anthropic" {
        return Ok(anthropic(facts, model, thinking_level));
    }
    if row.provider == "openai-responses-compatible" && wire == Wire::OpenaiResponses {
        return relay::responses(row, thinking_level, parallel.unwrap_or(true));
    }
    if let ProviderKind::OpenaiCompatible { name } = &route.kind {
        return compatible::chat(name, model, thinking_level, parallel);
    }
    if row.provider != "openai" {
        if thinking_level.is_some() || parallel == Some(false) {
            return Err(unavailable(
                "Thinking or parallel-call policy is not supported for this provider",
            ));
        }
        return Ok(json!({}));
    }
    // An explicit account capability can opt out; it never gates rendering.
    let summary = stored
        .and_then(|model| model.supports_reasoning_summary)
        .unwrap_or(false);
    let options = openai(wire, thinking_level, summary, parallel.unwrap_or(true));
    Ok(options)
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

fn openai(wire: Wire, level: Option<ThinkingLevel>, summary: bool, parallel: bool) -> Value {
    let mut options = json!({"store": false, "parallelToolCalls": parallel});
    if let Some(level) = level {
        options["reasoningEffort"] = match level {
            ThinkingLevel::Off => json!("none"),
            level => json!(level),
        };
    }
    if wire == Wire::OpenaiResponses && summary && level != Some(ThinkingLevel::Off) {
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
