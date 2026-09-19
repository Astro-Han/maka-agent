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

use super::control::{Result, failure};
use maka_protocol::{
    OperationErrorCode as Code,
    workhub::{AnswerInput, TurnResult},
};
use maka_runtime::{artifact::content_digest, execution::ToolMode};

/// Stable user request, independent of current model or plugin availability.
pub(crate) struct Request {
    pub input: AnswerInput,
    pub fingerprint: String,
}
impl Request {
    pub(crate) fn new(input: AnswerInput) -> Result<Self> {
        if input.text.trim().is_empty() {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub answer text is empty",
            ));
        }
        let fingerprint = serde_json::to_vec(&input)
            .map(|bytes| content_digest(&bytes))
            .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
        Ok(Self { input, fingerprint })
    }
}

/// One plugin generation's decision; Host checks its observed configuration
/// again and freezes this surface in the canonical opening.
pub(crate) struct Plan {
    pub request: Request,
    pub configuration_digest: String,
    pub control: super::Control,
    pub tool_mode: ToolMode,
    pub max_steps: usize,
}

impl super::Control {
    pub(crate) async fn answer(
        &self,
        request: Request,
        connection: uuid::Uuid,
    ) -> Result<TurnResult> {
        let _call = self
            .caller
            .admit()
            .map_err(|error| failure(Code::OperationUnavailable, error.to_string()))?;
        let session = self
            .commands
            .coordinator(self.caller.clone())
            .await?
            .ok_or_else(|| failure(Code::NotFound, "WorkHub Session has not been resolved"))?;
        let defaults = self.commands.chat_defaults().await?;
        self.commands
            .answer(
                self.caller.clone(),
                Plan {
                    request,
                    configuration_digest: session.configuration_digest,
                    control: self.clone(),
                    tool_mode: if defaults.code_mode_enabled {
                        ToolMode::CodeMode
                    } else {
                        ToolMode::Direct
                    },
                    max_steps: 64,
                },
                connection,
            )
            .await
    }
}
