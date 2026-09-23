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
use super::unavailable;
use maka_config::model_catalog::{ProviderFacts, resolve_limits};
use maka_protocol::OperationError;
use maka_runtime::{configuration::ConnectionCatalogEntry, context::ModelRequestContext};

pub(super) fn resolve(
    connection: &ConnectionCatalogEntry,
    facts: &ProviderFacts,
    model_id: &str,
) -> Result<ModelRequestContext, OperationError> {
    let capacity = resolve_limits(connection, facts, model_id)
        .map_err(|error| unavailable(error.to_string()))?;
    Ok(ModelRequestContext {
        provider_id: connection.provider_type.clone(),
        context_window: capacity.input_budget(),
        model_context_window: capacity.context_window,
        declared_window: connection
            .model_overrides
            .as_ref()
            .and_then(|values| values.get(model_id))
            .and_then(|value| value.compaction_threshold),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn full_window_input_budget_and_compaction_policy_remain_independent() {
        let mut connection: ConnectionCatalogEntry = serde_json::from_value(json!({
            "connectionId":"connection","revision":1,"slug":"fixture","name":"Fixture",
            "providerType":"openai","enabled":true,"enabledModelIds":["custom"],
            "models":[{"id":"custom","contextWindow":128000,"inputLimit":96000}],
            "modelOverrides":{"custom":{"compactionThreshold":64000}}
        }))
        .unwrap();
        let facts = maka_config::model_catalog::provider_facts("openai").unwrap();
        let context = resolve(&connection, facts, "custom").unwrap();
        assert_eq!(context.model_context_window, Some(128000));
        assert_eq!(context.context_window, Some(96000));
        assert_eq!(context.declared_window, Some(64000));
        connection.models[0].context_window = None;
        let context = resolve(&connection, facts, "custom").unwrap();
        assert_eq!(context.context_window, Some(96000));
        assert_eq!(
            context.model_context_window, None,
            "input-only limits cannot become full capacity"
        );
    }
}
