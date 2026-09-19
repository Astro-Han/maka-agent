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
use maka_event_log::{
    message_resolution::MessageExecution, observation::AssistantExcerpt, turns::InvocationState,
};
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::{event::InvocationOutcome, interaction::entity_id};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Reference {
    id: String,
    target_session_id: String,
    target_message_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Feedback {
    id: String,
    state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_preview: Option<String>,
}

#[derive(PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Accepted,
    Running,
    WaitingForUser,
    Completed,
    Failed,
    Aborted,
    Recovering,
}

impl super::Control {
    pub(super) async fn feedback(
        &self,
        references: Vec<Reference>,
        cancellation: CancellationToken,
    ) -> Result<Vec<Feedback>> {
        if references.len() > 64 {
            return Err(failure(
                Code::InvalidRequest,
                "WorkHub feedback accepts at most 64 references",
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for reference in &references {
            if reference.id.is_empty() || reference.id.len() > 512 || !ids.insert(&reference.id) {
                return Err(failure(
                    Code::InvalidRequest,
                    "Invalid WorkHub feedback identity",
                ));
            }
            for id in [&reference.target_session_id, &reference.target_message_id] {
                entity_id(id).map_err(|error| failure(Code::InvalidRequest, error))?;
            }
        }
        let _call = self
            .caller
            .admit()
            .map_err(|error| failure(Code::OperationUnavailable, error.to_string()))?;
        let mut result = Vec::with_capacity(references.len());
        for reference in references {
            super::control::check_request(&cancellation)?;
            let observation = self
                .commands
                .message_observation(reference.target_session_id, reference.target_message_id)
                .await?;
            let state = match observation.execution {
                MessageExecution::Missing => State::Recovering,
                MessageExecution::Pending => State::Accepted,
                MessageExecution::Cancelled => State::Aborted,
                MessageExecution::Owned(boundary) | MessageExecution::Shared(boundary) => {
                    match boundary.state {
                        InvocationState::Admitted => State::Accepted,
                        InvocationState::Running => State::Running,
                        InvocationState::WaitingForUser => State::WaitingForUser,
                        InvocationState::Ended { outcome, .. } => match outcome {
                            InvocationOutcome::Completed
                            | InvocationOutcome::ContextCompactFinished { .. } => State::Completed,
                            InvocationOutcome::Failed { .. } => State::Failed,
                            InvocationOutcome::Cancelled { .. } => State::Aborted,
                            InvocationOutcome::HandoffPaused { .. } => State::Recovering,
                        },
                    }
                }
            };
            let result_preview = (state == State::Completed)
                .then(|| observation.answer.and_then(preview))
                .flatten();
            result.push(Feedback {
                id: reference.id,
                state,
                result_preview,
            });
        }
        Ok(result)
    }
}

fn preview(answer: AssistantExcerpt) -> Option<String> {
    let normalized = answer.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    let mut text: String = normalized.chars().take(601).collect();
    if !answer.complete || text.chars().count() > 600 {
        text = text.chars().take(599).collect();
        text.push('…');
    }
    Some(text)
}
