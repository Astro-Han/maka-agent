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

mod output;

use crate::{Engine, RunCancellation, RunError, RunningInvocation};
use maka_plugins::executor::{Binding, Error, Outcome, Request};
use maka_runtime::{
    event::{EventWrite, Fact, InvocationInput, InvocationOutcome, RuntimeEvent},
    execution::InvocationConfiguration,
    message::RootSourceMessage,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct ExecutorInput {
    pub request: Request,
    pub binding: Binding,
    pub configuration: InvocationConfiguration,
    pub request_fingerprint: Option<String>,
    pub source_messages: Vec<RootSourceMessage>,
}

impl Engine {
    pub async fn start_executor(
        &self,
        input: ExecutorInput,
        cancellation: CancellationToken,
    ) -> Result<RunningInvocation, RunError> {
        if input.configuration.model.is_some() || input.configuration.cwd != input.request.cwd {
            return Err(RunError::InvalidInput(
                "executor cannot carry a model binding or mismatched workspace".into(),
            ));
        }
        maka_runtime::message::validate_opening(
            &input.request.content,
            &input.source_messages,
            None,
        )
        .map_err(|reason| RunError::InvalidInput(reason.into()))?;
        let identity = input
            .binding
            .identity()
            .map_err(|error| RunError::InvalidInput(error.to_string()))?;
        let call = input
            .binding
            .admit(input.request.clone())
            .map_err(|error| RunError::InvalidInput(error.to_string()))?;
        let inner = self.0.clone();
        self.start_owned(
            input.request.invocation.clone(),
            Arc::default(),
            None,
            cancellation,
            move |cancellation, admitted| async move {
                if cancellation.is_cancelled() {
                    return Err(RunError::Cancelled);
                }
                let invocation = input.request.invocation;
                // The durable backend identity precedes every external effect, in
                // the same transaction as the invocation admission.
                let writes = [
                    Fact::InvocationOpened {
                        configuration: Some(input.configuration),
                        input: InvocationInput::Message {
                            content: input.request.content,
                            request_fingerprint: input.request_fingerprint,
                            source_messages: input.source_messages,
                            skill_invocation: None,
                        },
                    },
                    Fact::ExecutorStarted { binding: identity },
                ]
                .into_iter()
                .map(|fact| EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)))
                .collect::<Result<Vec<_>, _>>()?;
                inner.log.append_batch(&writes).await?;
                let _ = admitted.send(());
                let output = Arc::new(output::Recorder::new(
                    inner.log.clone(),
                    invocation.clone(),
                    cancellation.token().clone(),
                ));
                let settlement = call
                    .execute(output.clone(), cancellation.token().clone())
                    .await;
                let outcome = outcome(&settlement.result, &cancellation);
                let committed = output.finish(&settlement.result, &outcome).await;
                drop(settlement); // the terminal fact owns the lease through its commit
                committed?;
                match outcome {
                    InvocationOutcome::Completed => Ok(invocation),
                    InvocationOutcome::Cancelled { .. } => Err(RunError::Cancelled),
                    InvocationOutcome::Failed { message, .. } => Err(RunError::Internal(
                        message.unwrap_or_else(|| "external executor failed".into()),
                    )),
                    _ => unreachable!("executor outcomes"),
                }
            },
        )
        .await
    }
}

fn outcome(result: &Result<Outcome, Error>, cancellation: &RunCancellation) -> InvocationOutcome {
    match result {
        Ok(Outcome::Completed { .. }) if !cancellation.is_cancelled() => {
            InvocationOutcome::Completed
        }
        Err(Error::Cancelled) | Ok(Outcome::Completed { .. }) => InvocationOutcome::Cancelled {
            source: cancellation.source(),
        },
        Ok(Outcome::Cancelled { .. }) => InvocationOutcome::Cancelled {
            source: "executor_provider".into(),
        },
        Err(Error::Retired) => InvocationOutcome::Cancelled {
            source: "executor_retired".into(),
        },
        Ok(Outcome::Failed { message, .. }) => InvocationOutcome::Failed {
            class: "executor".into(),
            message: Some(message.chars().take(2048).collect()),
        },
        Err(error) => InvocationOutcome::Failed {
            class: if matches!(error, Error::CleanupUnconfirmed) {
                "outcome_unknown"
            } else {
                "executor"
            }
            .into(),
            message: Some(error.to_string().chars().take(2048).collect()),
        },
    }
}
