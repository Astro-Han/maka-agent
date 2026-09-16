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

use crate::{archive::valid_projection_digest, context::ModelRequestContext};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// Continuation settings, not credentials or process-local resources.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffExecution {
    /// Admission projection, before any successor-side automatic compaction.
    pub replay: crate::continuation::ReplayEvidence,
    pub context: Option<ModelRequestContext>,
    /// Provider-specific options intentionally retain their extensible JSON shape.
    pub provider_options: Value,
    pub main_output_limit: Option<u64>,
    pub supports_vision: bool,
    pub tools: HandoffTools,
    // Keep the historical field name; old boolean facts remain readable.
    #[serde(rename = "compaction_attempted")]
    pub compaction: CompactionBudget,
    /// Manual continuation's stable-cut policy, absent for ordinary conversation.
    /// Physical handoff must not introduce or discard this projection policy.
    pub replay_base: Option<u64>,
}

/// A provider-accepted step renews reshaping, but not a failed summarizer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", from = "StoredCompactionBudget")]
pub enum CompactionBudget {
    Available,
    Reshaped,
    Failed,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredCompactionBudget {
    Legacy(bool),
    Current(CompactionState),
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CompactionState {
    Available,
    Reshaped,
    Failed,
}

impl From<StoredCompactionBudget> for CompactionBudget {
    fn from(value: StoredCompactionBudget) -> Self {
        match value {
            StoredCompactionBudget::Legacy(false)
            | StoredCompactionBudget::Current(CompactionState::Available) => Self::Available,
            StoredCompactionBudget::Current(CompactionState::Reshaped) => Self::Reshaped,
            // Legacy facts did not distinguish failure from successful reshaping.
            StoredCompactionBudget::Legacy(true)
            | StoredCompactionBudget::Current(CompactionState::Failed) => Self::Failed,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffTools {
    /// Covers definitions, nesting, execution semantics and discovery mode.
    /// It does not promise persistence of a handler's executable implementation.
    pub catalog_digest: String,
    pub loaded: BTreeSet<String>,
}

impl HandoffExecution {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        self.replay.validate()?;
        if !valid_projection_digest(&self.tools.catalog_digest)
            || self
                .main_output_limit
                .is_some_and(|n| n == 0 || n > 10_000_000_000)
            || self.tools.loaded.len() > 128
            || self
                .tools
                .loaded
                .iter()
                .any(|name| name.is_empty() || name.len() > 128)
        {
            return Err("invalid handoff execution settings");
        }
        if let Some(context) = &self.context {
            context.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::CompactionBudget;

    #[test]
    fn compaction_budget_round_trips_and_reads_legacy_handoffs() {
        for state in [
            CompactionBudget::Available,
            CompactionBudget::Reshaped,
            CompactionBudget::Failed,
        ] {
            let value = serde_json::to_value(state).unwrap();
            assert_eq!(
                serde_json::from_value::<CompactionBudget>(value).unwrap(),
                state
            );
        }
        assert_eq!(
            serde_json::from_str::<CompactionBudget>("false").unwrap(),
            CompactionBudget::Available
        );
        assert_eq!(
            serde_json::from_str::<CompactionBudget>("true").unwrap(),
            CompactionBudget::Failed
        );
        assert!(serde_json::from_str::<CompactionBudget>("1").is_err());
    }
}
