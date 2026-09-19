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

use super::{
    MAX_PREVIEW_BYTES, PreviewInput, PreviewOutcome, PreviewResult, invalid, revision, text,
};
use crate::Result;
use serde_json::Value;

pub fn decode_preview_input(value: &Value) -> Result<PreviewInput> {
    let input: PreviewInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    super::validate_workspace(&input.context.workspace)?;
    revision(&input.expected_revision)?;
    text(&input.reference, 512)?;
    Ok(input)
}
pub fn decode_preview_output(value: &Value) -> Result<PreviewResult> {
    // Decode the flattened wire envelope without allowing unknown fields through it.
    let mut payload = crate::codec::record(value, "Skill preview")?.clone();
    let workspace = payload
        .remove("resolvedWorkspace")
        .ok_or_else(|| invalid("Missing workspace"))?;
    let resolved_workspace = serde_json::from_value(workspace).map_err(invalid)?;
    super::validate_projection(&resolved_workspace)?;
    let outcome: PreviewOutcome =
        serde_json::from_value(Value::Object(payload)).map_err(invalid)?;
    if serde_json::to_vec(&outcome).map_err(invalid)?.len() > MAX_PREVIEW_BYTES {
        return Err(invalid("Skill preview exceeds byte limit"));
    }
    match &outcome {
        PreviewOutcome::Preview {
            revision: r,
            expected_current_sha256,
            expected_source_sha256,
            current_snippet,
            source_snippet,
            summary,
            ..
        } => {
            for hash in [r, expected_current_sha256, expected_source_sha256] {
                revision(hash)?;
            }
            if current_snippet.len() > 24 * 1024
                || source_snippet.len() > 24 * 1024
                || summary.current_line_count > 1024 * 1024 + 1
                || summary.source_line_count > 1024 * 1024 + 1
                || summary.changed_line_count
                    > summary.current_line_count.max(summary.source_line_count)
            {
                return Err(invalid("Invalid Skill preview bounds"));
            }
        }
        PreviewOutcome::RevisionConflict {
            expected_revision,
            actual_revision,
        } => {
            revision(expected_revision)?;
            revision(actual_revision)?;
        }
        PreviewOutcome::Rejected { .. } => {}
    }
    Ok(PreviewResult {
        outcome,
        resolved_workspace,
    })
}
