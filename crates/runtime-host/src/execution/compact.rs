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
use maka_protocol::{
    Operation, OperationErrorCode as Code,
    context::{ContextCompactInput, ContextCompactResult},
    turn::{ContextCompactionOutcome, TurnSnapshot, TurnState},
};
use maka_runtime::event::Invocation;
use sha2::{Digest, Sha256};
use uuid::Uuid;

impl Executions {
    pub(crate) async fn compact(
        self: &std::sync::Arc<Self>,
        input: ContextCompactInput,
    ) -> Result<ContextCompactResult> {
        let _admission = self.lock_admission().await;
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let fingerprint = format!(
            "sha256:{:x}",
            Sha256::digest(
                serde_json::to_vec(&(Operation::ContextCompact, &input)).map_err(internal)?
            )
        );
        if let Some(record) = self.recorded(&input.session_id, &input.turn_id).await? {
            if record.fingerprint.as_deref() != Some(&fingerprint) {
                return Err(failure(
                    Code::OperationConflict,
                    "Turn identity belongs to another request",
                ));
            }
            return result(record.snapshot);
        }
        if self
            .has_session_work(&input.session_id)
            .await
            .map_err(internal)?
        {
            return Err(failure(Code::SessionBusy, "Session has an active Run"));
        }
        let session = self
            .log
            .get_session::<SessionConfiguration>(&input.session_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
        if session.archived {
            return Err(failure(
                Code::SessionArchived,
                "Cannot compact an archived Session",
            ));
        }
        let provider = provider::resolve(
            &self.configuration,
            &self.oauth,
            &input.session_id,
            &session.configuration,
        )
        .await?;
        let snapshot = self
            .launch(RunInput {
                invocation: Invocation {
                    session_id: input.session_id,
                    turn_id: input.turn_id,
                    run_id: Uuid::new_v4().to_string(),
                    invocation_id: Uuid::new_v4().to_string(),
                },
                work: RunWork::ContextCompact,
                request_fingerprint: Some(fingerprint),
                provider: provider.config,
                provider_options: provider.options,
                main_output_limit: provider.main_output_limit,
                supports_vision: provider.supports_vision,
                context: Some(provider.context),
                configuration: session
                    .configuration
                    .invocation_configuration()
                    .await
                    .map_err(internal)?,
            })
            .await?;
        result(snapshot)
    }
}

fn result(turn: TurnSnapshot) -> Result<ContextCompactResult> {
    let outcome = match &turn.state {
        TurnState::Completed {
            context_compaction_outcome: Some(outcome),
            ..
        } => Some(outcome.clone()),
        TurnState::Failed {
            failure_message,
            failure_class,
            ..
        } => Some(ContextCompactionOutcome::Failed {
            reason: failure_message.as_ref().unwrap_or(failure_class).clone(),
        }),
        TurnState::Cancelled { abort_source, .. } => Some(ContextCompactionOutcome::Failed {
            reason: abort_source.clone(),
        }),
        TurnState::Completed { .. } => {
            return Err(failure(
                Code::InternalFailure,
                "Compact terminal lacks its durable outcome",
            ));
        }
        _ => None,
    };
    Ok(match outcome {
        Some(outcome) => ContextCompactResult::Finished { turn, outcome },
        None => ContextCompactResult::Started { turn },
    })
}
