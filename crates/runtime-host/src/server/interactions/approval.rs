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

use super::Interactions;
use maka_runtime::{
    interaction::{
        ClosureReason, Decision, GrantTarget, InteractionOutcome, InteractionRecord,
        InteractionRequest,
    },
    tool_call::ToolRejection,
};
use maka_tools::{ApprovalFuture, ToolCallContext};
use tokio_util::sync::CancellationToken;

impl Interactions {
    pub(super) fn approval(
        &self,
        target: GrantTarget,
        context: ToolCallContext,
        cancellation: CancellationToken,
        provider: CancellationToken,
    ) -> ApprovalFuture {
        let owner = self.clone();
        Box::pin(async move {
            let mut commits = owner.log.subscribe_commits();
            let Some((record, first)) = owner
                .establish(target, context, &cancellation, &provider)
                .await?
            else {
                return Ok(());
            };
            loop {
                let gate = owner.admission.lock().await;
                let current = owner
                    .log
                    .interaction(&record.request_id)
                    .await
                    .map_err(|error| failed(owner.store_failure(error).message))?
                    .ok_or_else(|| denied("Canonical Interaction disappeared"))?;
                if let Some(outcome) = current.outcome {
                    return match outcome {
                        InteractionOutcome::ClientCapabilityDecision {
                            decision: Decision::Allow,
                            ..
                        } => {
                            let InteractionRequest::ClientCapability { target, .. } =
                                &current.request
                            else {
                                owner.shutdown.cancel();
                                return Err(failed("Canonical approval request changed kind"));
                            };
                            if owner
                                .log
                                .client_capability_grant(&current.session_id, target)
                                .await
                                .map_err(|error| failed(owner.store_failure(error).message))?
                                .is_none()
                            {
                                owner.shutdown.cancel();
                                return Err(failed(
                                    "Allowed interaction has no canonical Session grant",
                                ));
                            }
                            Ok(())
                        }
                        InteractionOutcome::ClientCapabilityDecision {
                            decision: Decision::Deny,
                            ..
                        } => Err(denied("Client Capability approval was denied")),
                        InteractionOutcome::Closure { reason, .. } => Err(denied(&format!(
                            "Client Capability approval closed: {reason:?}"
                        ))),
                        InteractionOutcome::FormAnswer { .. }
                        | InteractionOutcome::PermissionsDecision { .. }
                        | InteractionOutcome::QuestionAnswer { .. } => {
                            owner.shutdown.cancel();
                            Err(failed("Canonical approval outcome changed kind"))
                        }
                    };
                }
                drop(gate);
                let reason = tokio::select! {
                    biased;
                    _ = owner.shutdown.cancelled() => ClosureReason::TurnStopped,
                    _ = cancellation.cancelled() => ClosureReason::ProducerCancelled,
                    _ = provider.cancelled() => ClosureReason::ProviderDisconnected,
                    changed = commits.changed() => {
                        changed.map_err(|_| denied("Interaction observation closed"))?;
                        continue;
                    }
                };
                // The first producer owns the shared prompt. A joining call's
                // cancellation stops only that wait, not the original prompt.
                if first {
                    let _gate = owner.admission.lock().await;
                    owner
                        .commit_outcome(
                            &record.request_id,
                            InteractionOutcome::Closure {
                                reason,
                                committed_at: crate::server::configuration::now()
                                    .map_err(failed)?,
                            },
                        )
                        .await
                        .map_err(|error| failed(error.message))?;
                }
                return if reason == ClosureReason::ProviderDisconnected {
                    Err(denied(
                        "Client Capability provider disconnected before approval",
                    ))
                } else {
                    Err(ToolRejection::Cancelled)
                };
            }
        })
    }
}
impl Interactions {
    async fn establish(
        &self,
        target: GrantTarget,
        context: ToolCallContext,
        cancellation: &CancellationToken,
        provider: &CancellationToken,
    ) -> Result<Option<(InteractionRecord, bool)>, ToolRejection> {
        let _gate = self.admission.lock().await;
        if self.shutdown.is_cancelled() || cancellation.is_cancelled() {
            return Err(ToolRejection::Cancelled);
        }
        if provider.is_cancelled() {
            return Err(denied("Client Capability provider disconnected"));
        }
        let invocation = &context.invocation;
        let observation = self
            .active_projection(invocation)
            .await
            .map_err(|error| failed(error.message))?;
        if self
            .log
            .client_capability_grant(&invocation.session_id, &target)
            .await
            .map_err(|error| failed(self.store_failure(error).message))?
            .is_some()
        {
            return Ok(None);
        }
        if !observation
            .session
            .configuration
            .approval_policy
            .allows(maka_sandbox::ApprovalKind::Client)
        {
            return Err(denied(
                "Client Capability requires approval, but the Session approval policy forbids prompting",
            ));
        }
        for pending in &observation.pending_interactions {
            let InteractionRequest::ClientCapability { target: other, .. } = &pending.request
            else {
                continue;
            };
            if target.provider_id == other.provider_id
                && target.contract_id == other.contract_id
                && target.capability == other.capability
                && target.scope == other.scope
            {
                return Ok(Some((pending.clone(), false)));
            }
        }
        let record = InteractionRecord {
            session_id: invocation.session_id.clone(),
            turn_id: invocation.turn_id.clone(),
            run_id: invocation.run_id.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
            created_at: crate::server::configuration::now().map_err(failed)?,
            request: InteractionRequest::ClientCapability {
                tool_use_id: context.tool_use_id(),
                target,
            },
            outcome: None,
        };
        Ok(Some((
            self.publish(observation, record)
                .await
                .map_err(|error| failed(error.message))?,
            true,
        )))
    }
}
fn denied(message: &str) -> ToolRejection {
    ToolRejection::PolicyDenied {
        message: message.into(),
    }
}
fn failed(error: impl std::fmt::Display) -> ToolRejection {
    ToolRejection::PreparationFailed {
        message: error.to_string(),
    }
}
