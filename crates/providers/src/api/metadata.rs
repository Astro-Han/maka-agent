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

use crate::facts::ProviderFacts;
use maka_runtime::configuration::{ModelCapabilities, ModelInfo};
use maka_runtime::execution::ThinkingLevel;

/// Account facts win field by field; explicit false and empty choices are facts.
pub(super) fn enrich(provider: &str, facts: &ProviderFacts, mut model: ModelInfo) -> ModelInfo {
    if let Some(known) = facts.models.get(&model.id) {
        let metadata = &known.metadata;
        macro_rules! fallback {
            ($($field:ident),* $(,)?) => { $(if model.$field.is_none() {
                model.$field = metadata.$field.clone();
            })* };
        }
        fallback!(
            display_name,
            description,
            context_window,
            input_limit,
            max_output_tokens,
            knowledge_cutoff,
            structured_output,
            last_updated,
            modalities
        );
        if model.thinking_levels.is_none() {
            model.thinking_levels = Some(known.entry.thinking_levels.clone());
            if provider == "openai"
                && known
                    .metadata
                    .thinking_options
                    .as_ref()
                    .and_then(|options| options.efforts.as_ref())
                    .is_some_and(|efforts| efforts.iter().any(|effort| effort == "medium"))
            {
                model.default_thinking_level =
                    model.default_thinking_level.or(Some(ThinkingLevel::Medium));
                model.supports_reasoning_summary = model.supports_reasoning_summary.or(Some(true));
            }
        }
        let mut defaults = metadata.capabilities.unwrap_or_default();
        defaults.chat = defaults.chat.or(Some(known.entry.can_use_as_chat_default));
        defaults.vision = defaults.vision.or(Some(known.entry.supports_vision));
        let caps = model.capabilities.get_or_insert_default();
        fill(caps, defaults);
    }
    if provider == "openai" || provider == "anthropic" {
        let caps = model.capabilities.get_or_insert_default();
        caps.web_search = caps.web_search.or(Some(true));
    }
    if provider == "anthropic" && claude_vision(&model.id) {
        let caps = model.capabilities.get_or_insert_default();
        caps.vision = caps.vision.or(Some(true));
    }
    model
}

fn claude_vision(id: &str) -> bool {
    let Some(mut tail) = id.strip_prefix("claude-") else {
        return false;
    };
    loop {
        for family in ["opus", "sonnet", "haiku", "fable"] {
            if let Some(rest) = tail.strip_prefix(family)
                && rest
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
            {
                return true;
            }
        }
        let Some((part, rest)) = tail.split_once('-') else {
            return false;
        };
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return false;
        }
        tail = rest;
    }
}

fn fill(caps: &mut ModelCapabilities, defaults: ModelCapabilities) {
    macro_rules! fallback {
        ($($field:ident),* $(,)?) => { $(caps.$field = caps.$field.or(defaults.$field);)* };
    }
    fallback!(
        chat,
        vision,
        reasoning,
        function_calling,
        parallel_tool_calls,
        image_generation,
        web_search
    );
}
