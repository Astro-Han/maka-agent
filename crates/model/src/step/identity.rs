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

use super::StepBuilder;
use crate::{ModelError, events::invalid};
use maka_runtime::model::ModelToolCall;

// runtime-store/tool_calls bounds raw and canonical `step:call` identities.
const TOOL_IDENTITY_MAX_BYTES: usize = 4096;
// Epoch 141 Session tool_start requires a nonempty name of at most 256 bytes.
const TOOL_NAME_MAX_BYTES: usize = 256;

impl StepBuilder {
    /// Bind acceptance to the durable step identity before consuming its stream.
    /// Raw provider IDs remain unchanged; only their canonical operation size is checked.
    pub fn for_step(step_id: &str) -> Result<Self, ModelError> {
        if step_id.is_empty() || step_id.len() > TOOL_IDENTITY_MAX_BYTES {
            return Err(invalid("invalid model step identity"));
        }
        Ok(Self {
            step_id_bytes: Some(step_id.len()),
            ..Self::default()
        })
    }

    pub(super) fn validate_call(&self, call: &ModelToolCall) -> Result<(), ModelError> {
        let max_id_bytes =
            TOOL_IDENTITY_MAX_BYTES.saturating_sub(self.step_id_bytes.map_or(0, |bytes| bytes + 1));
        if call.id.is_empty() || call.id.len() > max_id_bytes {
            return Err(invalid(
                "tool call identity exceeds durable operation bounds",
            ));
        }
        if call.name.is_empty() || call.name.len() > TOOL_NAME_MAX_BYTES {
            return Err(invalid("tool name must contain 1 to 256 UTF-8 bytes"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_runtime::model::{ModelEvent, ModelFinishReason, ModelPart, ModelUsage};
    use serde_json::json;

    fn call(id: String, name: String, provider_executed: bool) -> ModelEvent {
        ModelEvent::ToolCall(ModelToolCall {
            id,
            name,
            provider_executed,
            input: json!({}),
            provider_options: None,
        })
    }

    fn finish() -> ModelEvent {
        ModelEvent::Finished {
            reason: ModelFinishReason::ToolCalls,
            usage: ModelUsage::default(),
            provider_options: None,
        }
    }

    #[test]
    fn step_identity_budget_counts_bytes_without_underflow() {
        for step in [String::new(), "x".repeat(4097)] {
            assert!(StepBuilder::for_step(&step).is_err());
        }
        let mut builder = StepBuilder::for_step(&"x".repeat(4096)).unwrap();
        assert!(
            builder
                .push(call("x".into(), "echo".into(), false))
                .is_err()
        );
        let mut builder = StepBuilder::for_step("é").unwrap();
        builder
            .push(call("x".repeat(4093), "echo".into(), false))
            .unwrap();
        builder.push(finish()).unwrap();
        assert!(builder.finish().is_ok());
        let mut builder = StepBuilder::default();
        assert!(
            builder
                .push(call("x".repeat(4097), "echo".into(), false))
                .is_err()
        );
    }

    #[test]
    fn malformed_call_poisoning_prevents_completed_steps() {
        for provider in [false, true] {
            for (id, name) in [
                (String::new(), "echo".into()),
                ("id".into(), String::new()),
                ("x".repeat(4092), "echo".into()),
                ("id".into(), "é".repeat(129)),
            ] {
                let mut builder = StepBuilder::for_step("step").unwrap();
                assert!(builder.push(call(id, name, provider)).is_err());
                assert!(builder.push(finish()).is_err());
                assert!(builder.finish().is_err());
            }
        }
    }

    #[test]
    fn boundary_ids_remain_raw_and_provider_results_must_match() {
        let id = format!("{}: /é", "x".repeat(4086));
        assert_eq!(id.len(), 4091);
        let name = "é".repeat(128);
        for provider in [false, true] {
            let mut builder = StepBuilder::for_step("step").unwrap();
            builder
                .push(call(id.clone(), name.clone(), provider))
                .unwrap();
            if provider {
                builder
                    .push(ModelEvent::ProviderToolResult {
                        id: id.clone(),
                        name: name.clone(),
                        output: json!({}),
                        is_error: false,
                        provider_options: None,
                    })
                    .unwrap();
            }
            builder.push(finish()).unwrap();
            let step = builder.finish().unwrap();
            assert_eq!(step.tool_calls().next().unwrap().id, id);
            assert_eq!(step.tool_calls().next().unwrap().name, name);
            if provider {
                assert!(
                    matches!(&step.parts[1], ModelPart::ToolResult { id: result, .. } if result == &id)
                );
            }
        }
        let mut builder = StepBuilder::for_step("step").unwrap();
        builder.push(call(id.clone(), name, true)).unwrap();
        assert!(
            builder
                .push(ModelEvent::ProviderToolResult {
                    id,
                    name: "other".into(),
                    output: json!({}),
                    is_error: false,
                    provider_options: None,
                })
                .is_err()
        );
        assert!(builder.finish().is_err());
    }
}
