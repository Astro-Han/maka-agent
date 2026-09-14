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

//! Context compaction and latest completed request diagnostics wire contracts.
#[path = "context_diagnostics.rs"]
mod diagnostics;
use crate::{
    ProtocolError, Result, codec,
    turn::{self, ContextCompactionOutcome, TurnSnapshot},
};
pub use diagnostics::*;
use maka_runtime::context::CompactOutcome;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextCompactInput {
    pub session_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextCompactResult {
    Started {
        turn: TurnSnapshot,
    },
    Finished {
        turn: TurnSnapshot,
        outcome: ContextCompactionOutcome,
    },
}

pub fn decode_context_compact_input(value: &Value) -> Result<ContextCompactInput> {
    let input: ContextCompactInput = serde_json::from_value(value.clone())
        .map_err(|error| ProtocolError::invalid(error.to_string()))?;
    turn::entity(&input.session_id)?;
    turn::entity(&input.turn_id)?;
    Ok(input)
}

pub fn decode_context_compact_result(value: &Value) -> Result<ContextCompactResult> {
    let record = codec::record(value, "Context compact result")?;
    codec::shaped(record, &["kind", "turn"], &["outcome"])?;
    let turn = turn::decode_turn_snapshot(&record["turn"])?;
    match (record["kind"].as_str(), record.get("outcome")) {
        (Some("started"), None) => Ok(ContextCompactResult::Started { turn }),
        (Some("finished"), Some(outcome)) => Ok(ContextCompactResult::Finished {
            turn,
            outcome: turn::decode_context_compaction_outcome(outcome)?,
        }),
        _ => Err(ProtocolError::invalid("Invalid context compact result")),
    }
}

pub fn assert_compact_output_for_input(
    input: &ContextCompactInput,
    output: &ContextCompactResult,
) -> Result<()> {
    let (ContextCompactResult::Started { turn } | ContextCompactResult::Finished { turn, .. }) =
        output;
    if input.session_id != turn.session_id || input.turn_id != turn.turn_id {
        return Err(ProtocolError::invalid(
            "Context compact changed operation identity",
        ));
    }
    Ok(())
}

impl From<&CompactOutcome> for ContextCompactionOutcome {
    fn from(outcome: &CompactOutcome) -> Self {
        match outcome {
            CompactOutcome::Compacted { checkpoint_id } => Self::Compacted {
                checkpoint_id: checkpoint_id.clone(),
            },
            CompactOutcome::Unchanged { reason } => Self::Unchanged {
                reason: reason.clone(),
            },
            CompactOutcome::Failed { reason } => Self::Failed {
                reason: reason.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_outcomes_preserve_wire_fields() {
        for (canonical, wire) in [
            (
                CompactOutcome::Compacted {
                    checkpoint_id: "checkpoint".into(),
                },
                json!({"kind":"compacted","checkpointId":"checkpoint"}),
            ),
            (
                CompactOutcome::Unchanged {
                    reason: "no_history".into(),
                },
                json!({"kind":"unchanged","reason":"no_history"}),
            ),
            (
                CompactOutcome::Failed {
                    reason: "summary_invalid".into(),
                },
                json!({"kind":"failed","reason":"summary_invalid"}),
            ),
        ] {
            let mapped = ContextCompactionOutcome::from(&canonical);
            assert_eq!(serde_json::to_value(&mapped).unwrap(), wire);
            assert_eq!(
                turn::decode_context_compaction_outcome(&wire).unwrap(),
                mapped
            );
        }
    }
}
