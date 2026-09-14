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

use super::{ProviderFacts, provider_facts};
use crate::{ConfigError, Result};
use maka_runtime::configuration::ConnectionCatalogEntry;

#[derive(Debug, PartialEq)]
pub struct ModelLimits {
    pub context_window: Option<u64>,
    pub input_limit: Option<u64>,
}

impl ModelLimits {
    pub fn input_budget(&self) -> Option<u64> {
        self.context_window
            .into_iter()
            .chain(self.input_limit)
            .min()
    }
}

/// Capacity fields resolve independently; a compaction threshold is not capacity.
pub fn resolve_limits(
    row: &ConnectionCatalogEntry,
    facts: &ProviderFacts,
    id: &str,
) -> Result<ModelLimits> {
    let reported = row.models.iter().find(|model| model.id == id);
    let metadata = facts.models.get(id).map(|model| &model.metadata);
    let declared = row
        .model_overrides
        .as_ref()
        .and_then(|values| values.get(id));
    let resolve = |explicit: Option<u64>, reported: Option<u64>, metadata: Option<u64>| {
        explicit.or(reported).or(metadata)
    };
    let limits = ModelLimits {
        context_window: resolve(
            declared.and_then(|value| value.context_window),
            reported.and_then(|model| model.context_window),
            metadata.and_then(|model| model.context_window),
        ),
        input_limit: resolve(
            declared.and_then(|value| value.input_limit),
            reported.and_then(|model| model.input_limit),
            metadata.and_then(|model| model.input_limit),
        ),
    };
    if matches!((limits.context_window, limits.input_limit), (Some(context), Some(input)) if input > context)
    {
        return Err(ConfigError::Invalid(
            "Model input limit exceeds the context window. Update the model limits.".into(),
        ));
    }
    Ok(limits)
}

pub(crate) fn validate_overrides(row: &ConnectionCatalogEntry) -> Result<()> {
    if let Some(overrides) = &row.model_overrides {
        let facts = provider_facts(&row.provider_type)?;
        for id in overrides.keys() {
            resolve_limits(row, facts, id)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn independent_facts_do_not_turn_capacity_into_a_compaction_policy() {
        let mut row: ConnectionCatalogEntry = serde_json::from_value(json!({
            "connectionId":"connection","revision":1,"slug":"fixture","name":"Fixture",
            "providerType":"openai","enabled":true,"enabledModelIds":["custom"],
            "models":[{"id":"custom","contextWindow":6000,"inputLimit":4000}],
            "modelOverrides":{"custom":{"contextWindow":8000,"compactionThreshold":9000}}
        }))
        .unwrap();
        let facts = provider_facts("openai").unwrap();
        assert_eq!(
            resolve_limits(&row, facts, "custom").unwrap(),
            ModelLimits {
                context_window: Some(8000),
                input_limit: Some(4000)
            }
        );
        let declaration = row
            .model_overrides
            .as_mut()
            .unwrap()
            .get_mut("custom")
            .unwrap();
        declaration.context_window = Some(3000);
        assert!(validate_overrides(&row).is_err());
        row.model_overrides
            .as_mut()
            .unwrap()
            .get_mut("custom")
            .unwrap()
            .input_limit = Some(2000);
        assert_eq!(
            resolve_limits(&row, facts, "custom")
                .unwrap()
                .input_budget(),
            Some(2000)
        );
    }
}
