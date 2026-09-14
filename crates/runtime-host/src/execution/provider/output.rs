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
use maka_config::model_catalog::ProviderFacts;
use maka_protocol::OperationError;
use maka_runtime::configuration::ConnectionCatalogEntry;
use serde_json::Value;

/// An explicit reply budget is capped by capacity, before subtracting a fixed
/// Anthropic thinking budget. Other OpenAI routes have no implicit reply cap.
pub(super) fn resolve(
    connection: &ConnectionCatalogEntry,
    facts: &ProviderFacts,
    model_id: &str,
    wire: Wire,
    options: &Value,
) -> Result<Option<u64>, OperationError> {
    let requested = connection
        .model_overrides
        .as_ref()
        .and_then(|overrides| overrides.get(model_id))
        .and_then(|value| value.max_output_tokens);
    if requested.is_none()
        && wire != Wire::AnthropicMessages
        && !(connection.provider_type == "kimi-coding-plan" && wire == Wire::OpenaiChat)
    {
        return Ok(None);
    }
    let model = connection.models.iter().find(|model| model.id == model_id);
    let limit = model.and_then(|model| model.max_output_tokens).or_else(|| {
        facts
            .models
            .get(model_id)
            .and_then(|model| model.metadata.max_output_tokens)
    });
    let limit = match (requested, limit) {
        (Some(requested), Some(capacity)) => Some(requested.min(capacity)),
        (requested, capacity) => requested.or(capacity),
    };
    limit
        .map(|limit| {
            if wire == Wire::AnthropicMessages {
                subtract_budget(limit, options)
            } else {
                Ok(limit)
            }
        })
        .transpose()
}

fn subtract_budget(limit: u64, options: &Value) -> Result<u64, OperationError> {
    let thinking = &options["anthropic"]["thinking"];
    let fixed = if thinking["type"] == "enabled" {
        thinking["budgetTokens"].as_u64().unwrap_or(0)
    } else {
        0
    };
    limit
        .checked_sub(fixed)
        .filter(|limit| *limit > 0)
        .ok_or_else(|| unavailable("Model output limit does not leave a positive text budget"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_config::model_catalog::provider_facts;
    use serde_json::json;

    #[test]
    fn main_limit_uses_effective_row_only_on_supported_wire_and_subtracts_fixed_budget() {
        let mut row: ConnectionCatalogEntry = serde_json::from_value(json!({
            "connectionId":"connection","revision":1,"slug":"fixture","name":"Fixture",
            "providerType":"anthropic","enabled":true,"enabledModelIds":["claude-custom"],
            "models":[{"id":"claude-custom","maxOutputTokens":10_000_000_000_u64}]
        }))
        .unwrap();
        let facts = provider_facts("anthropic").unwrap();
        let fixed = json!({"anthropic":{"thinking":{"type":"enabled","budgetTokens":1024}}});
        assert_eq!(
            resolve(
                &row,
                facts,
                "claude-custom",
                Wire::AnthropicMessages,
                &fixed
            )
            .unwrap(),
            Some(9_999_998_976)
        );
        for wire in [Wire::OpenaiChat, Wire::OpenaiResponses] {
            assert_eq!(
                resolve(&row, facts, "claude-custom", wire, &fixed).unwrap(),
                None
            );
        }
        row.model_overrides = Some(
            serde_json::from_value(json!({
                "claude-custom":{"maxOutputTokens":5000}
            }))
            .unwrap(),
        );
        for wire in [
            Wire::OpenaiChat,
            Wire::OpenaiResponses,
            Wire::AnthropicMessages,
        ] {
            assert_eq!(
                resolve(&row, facts, "claude-custom", wire, &fixed).unwrap(),
                Some(if wire == Wire::AnthropicMessages {
                    3976
                } else {
                    5000
                })
            );
        }
        row.models[0].max_output_tokens = Some(4000);
        assert_eq!(
            resolve(
                &row,
                facts,
                "claude-custom",
                Wire::AnthropicMessages,
                &fixed
            )
            .unwrap(),
            Some(2976),
            "reply budget cannot exceed reported capacity"
        );
        for kind in ["adaptive", "disabled"] {
            assert_eq!(
                subtract_budget(
                    5000,
                    &json!({"anthropic":{"thinking":{"type":kind,"budgetTokens":1024}}})
                )
                .unwrap(),
                5000
            );
        }
        assert!(subtract_budget(1024, &fixed).is_err());
        assert!(subtract_budget(1000, &fixed).is_err());
    }
}
