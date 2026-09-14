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

use crate::{Inner, RunError, RunInput, RunWork, compact, prune, steps};
use maka_event_log::StoreError;
use maka_runtime::event::{
    CommitError, EventSink, EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent,
};
use maka_runtime::input::InvocationInput;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub async fn run(
    inner: Arc<Inner>,
    input: RunInput,
    cancellation: CancellationToken,
    admitted: tokio::sync::oneshot::Sender<()>,
) -> Result<Invocation, RunError> {
    if cancellation.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    match inner
        .log
        .prepare_prune_candidates(&input.invocation.session_id, None, 0, 0, None)
        .await
    {
        Ok(_) => {}
        Err(StoreError::InvalidTransition(reason)) => {
            return Err(RunError::ReconciliationRequired(reason));
        }
        Err(error) => return Err(error.into()),
    }
    let opening = match &input.work {
        RunWork::Message {
            message,
            source_messages,
            skill_invocation,
            ..
        } => InvocationInput::Message {
            content: message.clone(),
            request_fingerprint: input.request_fingerprint.clone(),
            source_messages: source_messages.clone(),
            skill_invocation: skill_invocation.clone(),
        },
        RunWork::ContextCompact => InvocationInput::ContextCompact {
            request_fingerprint: input
                .request_fingerprint
                .clone()
                .expect("validated compact fingerprint"),
        },
    };
    append(
        &inner,
        &input.invocation,
        Fact::InvocationOpened {
            configuration: Some(input.configuration.clone()),
            input: opening,
        },
    )
    .await?;
    let _ = admitted.send(());
    let result = async {
        prune::run(&inner, &input, &cancellation).await?;
        match &input.work {
            RunWork::Message {
                tools, max_steps, ..
            } => steps::run(&inner, &input, tools, *max_steps, &cancellation)
                .await
                .map(|()| (InvocationOutcome::Completed, None)),
            RunWork::ContextCompact => compact::run(
                &inner,
                &input,
                &maka_runtime::context::CheckpointMode::Standalone,
                &cancellation,
            )
            .await
            .map(|(outcome, checkpoint)| {
                (
                    InvocationOutcome::ContextCompactFinished { outcome },
                    checkpoint,
                )
            }),
        }
    }
    .await;
    let result = if result.is_ok() && cancellation.is_cancelled() {
        Err(RunError::Cancelled)
    } else {
        result
    };
    let (outcome, checkpoint) = match &result {
        Ok((outcome, checkpoint)) => (outcome.clone(), checkpoint.clone()),
        Err(RunError::Cancelled | RunError::Model(maka_model::ModelError::Cancelled)) => (
            InvocationOutcome::Cancelled {
                source: "runtime_cancellation".into(),
            },
            None,
        ),
        Err(error) => (
            InvocationOutcome::Failed {
                class: failure_class(error).into(),
                message: Some(error.to_string().chars().take(2048).collect()),
            },
            None,
        ),
    };
    if let Some(checkpoint) = checkpoint {
        // A checkpoint is adopted only with its successful terminal in this transaction.
        let terminal =
            RuntimeEvent::new(input.invocation.clone(), Fact::InvocationEnded { outcome });
        let committed = inner
            .log
            .append_batch(&[EventWrite::plain(checkpoint)?, EventWrite::plain(terminal)?])
            .await;
        if let Err(error) = committed {
            if matches!(error, CommitError::Rejected(_)) {
                append(
                    &inner,
                    &input.invocation,
                    Fact::InvocationEnded {
                        outcome: InvocationOutcome::Failed {
                            class: "event_commit".into(),
                            message: Some(error.to_string().chars().take(2048).collect()),
                        },
                    },
                )
                .await?;
            }
            return Err(error.into());
        }
    } else {
        append(&inner, &input.invocation, Fact::InvocationEnded { outcome }).await?;
    }
    result?;
    Ok(input.invocation)
}

fn failure_class(error: &RunError) -> &'static str {
    match error {
        RunError::Busy => "session_busy",
        RunError::ReconciliationRequired(_) => "reconciliation_required",
        RunError::InvalidInput(_) => "invalid_input",
        RunError::Cancelled | RunError::Model(maka_model::ModelError::Cancelled) => "cancelled",
        RunError::StepLimit => "step_limit",
        RunError::Commit(_) => "event_commit",
        RunError::Store(_) => "event_store",
        RunError::Model(maka_model::ModelError::TimedOut) => "model_timeout",
        RunError::Model(maka_model::ModelError::Adapter(_)) => "model_adapter",
        RunError::Model(maka_model::ModelError::ContextOverflow { .. }) => "context_overflow",
        RunError::Tool(_) => "tool_execution",
        RunError::Internal(_) => "runtime_internal",
    }
}

pub(super) async fn append(
    inner: &Inner,
    invocation: &Invocation,
    fact: Fact,
) -> Result<(), RunError> {
    let log = inner.log.clone();
    let event = RuntimeEvent::new(invocation.clone(), fact);
    if matches!(event.fact, Fact::InvocationEnded { .. }) {
        let now = event
            .recorded_at
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| CommitError::Rejected(error.to_string()))?
            .as_millis();
        log.close_run_interactions(
            invocation,
            maka_runtime::interaction::ClosureReason::TurnTerminal,
            u64::try_from(now).map_err(|error| CommitError::Rejected(error.to_string()))?,
        )
        .await
        .map_err(|error| match error {
            maka_event_log::StoreError::CommitUnknown(error) => {
                CommitError::OutcomeUnknown(error.to_string())
            }
            maka_event_log::StoreError::OperationUnknown => {
                CommitError::OutcomeUnknown(error.to_string())
            }
            other => CommitError::Rejected(other.to_string()),
        })?;
    }
    log.commit(maka_runtime::event::EventWrite::plain(event)?)
        .await?;
    Ok(())
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
