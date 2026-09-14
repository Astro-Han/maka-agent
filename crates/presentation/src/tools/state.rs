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

use crate::{ProjectionError, ToolMetadata};
use maka_runtime::event::RuntimeEvent;
use maka_runtime::tool_call::{ToolCallIdentity, ToolOrigin};
use serde_json::Value;
use std::collections::HashMap;

pub(super) struct Pending {
    pub(super) identity: ToolCallIdentity,
    pub(super) name: String,
    pub(super) input: Value,
    pub(super) metadata: ToolMetadata,
    pub(super) state: CallState,
    pub(super) bytes: usize,
}
#[derive(PartialEq)]
pub(super) enum CallState {
    AwaitingDispatch,
    Dispatched,
    ProviderExecuted,
}
pub(crate) struct Tools {
    pub(super) pending: HashMap<String, Pending>,
    pub(super) bytes: usize,
    pub(super) limit: usize,
}
impl Tools {
    pub fn new(limit: usize) -> Self {
        Self {
            pending: HashMap::new(),
            bytes: 0,
            limit,
        }
    }
    pub(super) fn insert(
        &mut self,
        operation: &str,
        identity: ToolCallIdentity,
        name: &str,
        input: &Value,
        state: CallState,
        event: &RuntimeEvent,
    ) -> Result<ToolMetadata, ProjectionError> {
        if self.pending.contains_key(operation) {
            return Err(ProjectionError::Invalid("duplicate tool call"));
        }
        if let ToolOrigin::CodeMode {
            parent_operation_id,
            parent_tool_call_id,
        } = &identity.origin
        {
            let parent = self
                .pending
                .get(parent_operation_id)
                .ok_or(ProjectionError::Invalid("nested call without parent"))?;
            if parent.state != CallState::Dispatched
                || parent.identity.tool_call_id != *parent_tool_call_id
            {
                return Err(ProjectionError::Invalid("nested call parent mismatch"));
            }
        }
        let metadata =
            ToolMetadata::from_origin(&identity.origin, &event.invocation.invocation_id)?;
        let bytes = serde_json::to_vec(&(operation, &identity, name, input))
            .map_err(|_| ProjectionError::TooLarge)?
            .len();
        if self.pending.len() >= 256 || bytes > self.limit.saturating_sub(self.bytes) {
            return Err(ProjectionError::TooLarge);
        }
        self.bytes += bytes;
        self.pending.insert(
            operation.into(),
            Pending {
                identity,
                name: name.into(),
                input: input.clone(),
                metadata: metadata.clone(),
                state,
                bytes,
            },
        );
        Ok(metadata)
    }
}
