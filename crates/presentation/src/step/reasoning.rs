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

use serde::Deserialize;
use serde_json::Value;

/// Provider-owned finalization evidence, not a successful model request.
pub(super) fn finalized(options: Option<&Value>) -> bool {
    let Some(options) = options else { return false };
    let anthropic = &options["anthropic"];
    if nonempty(&anthropic["signature"]) || anthropic["redactedData"].is_string() {
        return true;
    }
    let openai = &options["openai"];
    if nonempty(&openai["itemId"]) && nonempty(&openai["reasoningEncryptedContent"]) {
        return true;
    }
    let Ok(state) = Plaintext::deserialize(&options["makaResponses"]) else {
        return false;
    };
    state.version == 1
        && safe_id(&state.profile, 128)
        && safe_id(&state.item_id, 512)
        && state.summary_part_lengths.len() <= 128
        && state
            .summary_part_lengths
            .iter()
            .try_fold(0u64, |sum, part| {
                sum.checked_add(*part).filter(|total| *total <= 10_000_000)
            })
            .is_some()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Plaintext {
    version: u8,
    profile: String,
    item_id: String,
    summary_part_lengths: Vec<u64>,
}

fn nonempty(value: &Value) -> bool {
    value.as_str().is_some_and(|value| !value.is_empty())
}

fn safe_id(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.encode_utf16().count() <= limit
        && !value
            .chars()
            .any(|character| character <= '\u{1f}' || character == '\u{7f}')
}
