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

use super::{MutateInput, MutationOutcome, MutationResult, invalid, revision, text};
use crate::Result;
use serde_json::Value;

pub fn decode_mutate_input(value: &Value) -> Result<MutateInput> {
    let input: MutateInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    super::validate_workspace(&input.context.workspace)?;
    revision(&input.expected_revision)?;
    if let Some(reference) = input.mutation.reference() {
        text(reference, 512)?;
    }
    if let super::Mutation::Install { source_id, .. } = &input.mutation
        && !maka_skills::safe_source_id(source_id)
    {
        return Err(invalid("Invalid Skill source identity"));
    }
    Ok(input)
}
pub fn decode_mutate_output(value: &Value) -> Result<MutationResult> {
    let mut payload = crate::codec::record(value, "Skill mutation")?.clone();
    let workspace = payload
        .remove("resolvedWorkspace")
        .ok_or_else(|| invalid("Missing workspace"))?;
    let resolved_workspace = serde_json::from_value(workspace).map_err(invalid)?;
    super::validate_projection(&resolved_workspace)?;
    if matches!(
        payload.get("kind").and_then(Value::as_str),
        Some("committed" | "unchanged")
    ) && !payload.contains_key("entry")
    {
        return Err(invalid("Missing Skill mutation entry"));
    }
    let outcome: MutationOutcome =
        serde_json::from_value(Value::Object(payload)).map_err(invalid)?;
    match &outcome {
        MutationOutcome::Committed { revision: r, entry }
        | MutationOutcome::Unchanged { revision: r, entry } => {
            revision(r)?;
            if let Some(super::MutationEntry::Skill(entry)) = entry {
                super::governance::validate(entry)?;
            }
        }
        MutationOutcome::RevisionConflict {
            expected_revision,
            actual_revision,
        } => {
            revision(expected_revision)?;
            revision(actual_revision)?;
        }
        MutationOutcome::Rejected { .. } => {}
    }
    Ok(MutationResult {
        outcome,
        resolved_workspace,
    })
}
