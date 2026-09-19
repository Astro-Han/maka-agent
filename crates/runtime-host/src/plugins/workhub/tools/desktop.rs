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

use super::failed;
use crate::plugins::workhub::Control;
use maka_runtime::{
    capability::{CallResult, ContentBlock},
    tools::{ToolCallContext, ToolError},
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

pub(crate) const CONTEXT_TOOL: &str = "mcp__desktop_workhub__context";
pub(crate) const CONTROL_TOOL: &str = "mcp__desktop_workhub__control";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Creation {
    pub workspace: maka_protocol::session::WorkspaceTarget,
    pub defaults: maka_runtime::workhub::CreateDefaults,
}
pub(super) async fn creation(
    control: &Control,
    context: ToolCallContext,
    cancellation: CancellationToken,
) -> Result<Creation, ToolError> {
    let value = control
        .commands
        .client_call(
            control.caller.clone(),
            context,
            maka_plugins::client_capability::Call {
                name: CONTEXT_TOOL.into(),
                input: Default::default(),
            },
            cancellation,
        )
        .await?;
    let result: CallResult = serde_json::from_value(value).map_err(failed)?;
    if let Some(value) = result.structured_content {
        return serde_json::from_value(value).map_err(failed);
    }
    match result.content.as_slice() {
        [ContentBlock::Text { text }] => serde_json::from_str(text).map_err(failed),
        _ => Err(failed("Desktop returned no creation context")),
    }
}
