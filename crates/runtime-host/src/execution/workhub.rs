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

use super::{Executions, Result, failure, internal, provider};
use crate::session::SessionConfiguration;
use maka_agent::{RunInput, RunWork};
use maka_event_log::sessions::SessionRecord;
use maka_protocol::{
    OperationErrorCode as Code,
    workhub::{AnswerInput, TurnResult},
};
use maka_runtime::workhub::ActionId;
use maka_runtime::{artifact::content_digest, event::Invocation, workhub::COORDINATION_SESSION_ID};
use std::sync::Arc;
use uuid::Uuid;

pub(super) mod profile;

impl Executions {
    /// Caller owns admission; waiting for cleanup must happen after releasing it.
    pub(crate) async fn stop_workhub_owner(
        &self,
        owner: &Invocation,
        action_id: &ActionId,
    ) -> Result<Option<tokio_util::sync::CancellationToken>> {
        self.retire_owner(
            owner,
            maka_agent::CancellationCause::WorkhubStop {
                action_id: action_id.clone(),
            },
        )
        .await
    }

    pub(crate) async fn workhub_source(
        &self,
        turn: &str,
    ) -> Result<maka_event_log::turns::TurnBoundary> {
        let invocation = self
            .active
            .lock()
            .unwrap()
            .values()
            .find(|run| {
                run.invocation.session_id == COORDINATION_SESSION_ID
                    && run.invocation.turn_id == turn
                    && !run.cancellation.is_cancelled()
            })
            .map(|run| run.invocation.clone())
            .ok_or_else(|| failure(Code::OperationConflict, "WorkHub Turn is not active"))?;
        let boundary = self
            .log
            .run_boundary(COORDINATION_SESSION_ID, &invocation.run_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::OperationConflict, "WorkHub opening is missing"))?;
        if matches!(
            boundary.state,
            maka_event_log::turns::InvocationState::Ended { .. }
        ) {
            return Err(failure(Code::OperationConflict, "WorkHub Turn has ended"));
        }
        Ok(boundary)
    }

    /// The WorkHub entry point holds the shared admission gate and proves the
    /// stable Session identity before calling this method.
    pub(crate) async fn answer_workhub(
        self: &Arc<Self>,
        input: AnswerInput,
        session: SessionRecord<SessionConfiguration>,
        connection_id: Uuid,
        root_id: &str,
    ) -> Result<TurnResult> {
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        if input.text.trim().is_empty() {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub answer text is empty",
            ));
        }
        let fingerprint = content_digest(&serde_json::to_vec(&input).map_err(internal)?);
        if let Some(record) = self
            .recorded(COORDINATION_SESSION_ID, &input.turn_id)
            .await?
        {
            if record.fingerprint.as_deref() != Some(&fingerprint) {
                return Err(failure(
                    Code::OperationConflict,
                    "WorkHub Turn belongs to another request",
                ));
            }
            return Ok(TurnResult {
                turn_id: input.turn_id,
            });
        }
        if self
            .has_session_work(COORDINATION_SESSION_ID)
            .await
            .map_err(internal)?
        {
            return Err(failure(
                Code::SessionBusy,
                "WorkHub has active or pending work",
            ));
        }
        let content = input.content();
        self.validate_message_content(COORDINATION_SESSION_ID, &content, root_id)
            .await?;
        let (tools, composition) = profile::tools(self, &session.configuration, connection_id)?;
        let provider = provider::resolve(
            &self.configuration,
            &self.oauth,
            COORDINATION_SESSION_ID,
            &session.configuration,
        )
        .await?;
        let mut configuration = session
            .configuration
            .invocation_configuration()
            .await
            .map_err(internal)?;
        configuration.tool_mode = if self
            .configuration
            .runtime_policy()
            .await
            .map_err(internal)?
            .policy
            .chat_defaults
            .code_mode_enabled
        {
            maka_runtime::execution::ToolMode::CodeMode
        } else {
            maka_runtime::execution::ToolMode::Direct
        };
        configuration.system_prompt = Some(profile::prompt());
        configuration.tool_composition = Some(composition);
        let run = RunInput {
            invocation: Invocation {
                session_id: COORDINATION_SESSION_ID.into(),
                turn_id: input.turn_id.clone(),
                run_id: Uuid::new_v4().to_string(),
                invocation_id: Uuid::new_v4().to_string(),
            },
            work: RunWork::Message {
                source_messages: Vec::new(),
                skill_invocation: None,
                message: content.into(),
                tools,
                max_steps: 64,
            },
            request_fingerprint: Some(fingerprint),
            provider: provider.config,
            provider_options: provider.options,
            main_output_limit: provider.main_output_limit,
            supports_vision: provider.supports_vision,
            context: Some(provider.context),
            configuration,
        };
        self.launch(run).await?;
        Ok(TurnResult {
            turn_id: input.turn_id,
        })
    }
}
