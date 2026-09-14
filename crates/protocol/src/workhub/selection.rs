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

use super::{ActResult, candidates::text};
use crate::{ProtocolError, Result};
use maka_runtime::workhub::ActionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionInput {
    pub turn_id: String,
    pub action_id: ActionId,
    pub candidate_set_id: String,
    pub candidate_refs: Vec<String>,
    pub delegation_text: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionResult {
    Cancelled,
    Delegated { result: ActResult },
}

pub fn decode_selection(value: &Value) -> Result<SelectionInput> {
    let input: SelectionInput = crate::turn::decode(value)?;
    crate::turn::entity(&input.turn_id)?;
    text(&input.candidate_set_id, 256)?;
    text(&input.delegation_text, 48 * 1024)?;
    let mut seen = HashSet::new();
    if !(1..=32).contains(&input.candidate_refs.len()) {
        return Err(ProtocolError::invalid(
            "Invalid WorkHub selection candidates",
        ));
    }
    for candidate in &input.candidate_refs {
        crate::turn::entity(candidate)?;
        if !seen.insert(candidate) {
            return Err(ProtocolError::invalid(
                "Duplicate WorkHub selection candidate",
            ));
        }
    }
    Ok(input)
}
