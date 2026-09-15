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

use crate::{Inner, RunError, RunInput, auto_context, model_attempt, prune};
use futures_util::FutureExt;
use maka_runtime::{context::ModelPurpose, tools::ToolError};
use maka_tools::RunTools;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) async fn run(
    inner: &Arc<Inner>,
    input: &RunInput,
    catalog: &maka_tools::ToolCatalog,
    max_steps: usize,
    cancellation: &CancellationToken,
    continuation_base: Option<u64>,
    handoff: &crate::HandoffGate,
) -> Result<maka_runtime::event::InvocationOutcome, RunError> {
    let lane = maka_model::ResponsesLane::default();
    let tools = RunTools::new(
        inner.log.clone(),
        input.invocation.clone(),
        catalog.clone(),
        input.configuration.tool_mode,
        inner.cells.clone(),
    );
    let mut attempted = false;
    // This tracks work after this physical opening, not after the logical root.
    // A successor may compact the sealed prefix with a PreTurn boundary.
    let mut completed_step = false;
    if let crate::RunWork::Handoff { pause, .. } = &input.work {
        tools.restore(&pause.execution.tools)?;
        attempted = pause.execution.compaction_attempted;
    }
    for step in 0..max_steps {
        if cancellation.is_cancelled() {
            return Err(RunError::Cancelled);
        }
        inner.log.commit_pending_steering(&input.invocation).await?;
        if let Some(pause) = handoff
            .boundary(cancellation, |intent| async {
                let source = inner
                    .log
                    .read_model_context(
                        &input.invocation.session_id,
                        Some(&input.invocation.invocation_id),
                        10_000,
                        8 * 1024 * 1024,
                    )
                    .await
                    .ok()?;
                // Read the live source, but project from the successor's point of
                // view: manual replay cuts distinguish current from inherited work.
                let prompt = model_attempt::prompt(
                    inner,
                    input,
                    &source,
                    ModelPurpose::Main,
                    cancellation,
                    continuation_base,
                    &intent.successor_invocation_id,
                )
                .await
                .ok()?;
                let replay = crate::continuation::replay(
                    input,
                    prompt,
                    tools.capture().definitions(),
                    cancellation,
                )
                .ok()?;
                let pause = maka_runtime::handoff::HandoffPause {
                    intent,
                    remaining_steps: std::num::NonZeroU16::new((max_steps - step) as u16)
                        .expect("validated step budget"),
                    execution: Box::new(maka_runtime::handoff::HandoffExecution {
                        replay,
                        context: input.context.clone(),
                        provider_options: input.provider_options.clone(),
                        main_output_limit: input.main_output_limit,
                        supports_vision: input.supports_vision,
                        tools: tools.checkpoint(),
                        compaction_attempted: attempted,
                        replay_base: continuation_base,
                    }),
                };
                inner
                    .log
                    .check_handoff(&input.invocation, &pause)
                    .await
                    .ok()?;
                Some(pause)
            })
            .await
        {
            return Ok(maka_runtime::event::InvocationOutcome::HandoffPaused { pause });
        }
        if cancellation.is_cancelled() {
            return Err(RunError::Cancelled);
        }
        let mut source = inner
            .log
            .read_model_context(
                &input.invocation.session_id,
                Some(&input.invocation.invocation_id),
                10_000,
                8 * 1024 * 1024,
            )
            .await?;
        if !attempted && auto_context::due(input, &source) {
            attempted = true;
            if auto_context::attempt(
                inner,
                input,
                &source,
                completed_step,
                cancellation,
                continuation_base,
            )
            .await?
            {
                tools.clear_loaded();
            }
            source = inner
                .log
                .read_model_context(
                    &input.invocation.session_id,
                    Some(&input.invocation.invocation_id),
                    10_000,
                    8 * 1024 * 1024,
                )
                .await?;
        }
        let request_tools = tools.capture();
        let prompt = model_attempt::prompt(
            inner,
            input,
            &source,
            ModelPurpose::Main,
            cancellation,
            continuation_base,
            &input.invocation.invocation_id,
        )
        .await?;
        let result = model_attempt::execute(
            inner,
            input,
            &source,
            prompt,
            request_tools.definitions(),
            model_attempt::Attempt::Main {
                lane: lane.clone(),
                continuation_base,
            },
            cancellation,
        )
        .await;
        let (step_id, output) = match result {
            Ok(output) => output,
            Err(
                error @ RunError::Model(maka_model::ModelError::ContextOverflow {
                    observed_output: false,
                }),
            ) if !attempted && step + 1 < max_steps && !cancellation.is_cancelled() => {
                attempted = true;
                if auto_context::attempt(
                    inner,
                    input,
                    &source,
                    completed_step,
                    cancellation,
                    continuation_base,
                )
                .await?
                {
                    tools.clear_loaded();
                    continue;
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let local_calls: Vec<_> = output
            .tool_calls()
            .filter(|call| !call.provider_executed)
            .collect();
        if local_calls.is_empty() {
            return Ok(maka_runtime::event::InvocationOutcome::Completed);
        }
        let mut step_tools = request_tools.into_step(&step_id);
        for call in &local_calls {
            let result = std::panic::AssertUnwindSafe(async {
                step_tools.invoke(call, cancellation.clone()).await
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err(ToolError::OutcomeUnknown("tool panicked".into())));
            match result {
                Ok(_) | Err(ToolError::Failed(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        // Drain and persist all tool outcomes before rewriting the next model view.
        // Bound the view before either Responses confirmation or compaction reads it.
        prune::run(inner, input, cancellation).await?;
        if step + 1 < max_steps && !cancellation.is_cancelled() && lane.needs_confirmation() {
            let source = inner
                .log
                .read_model_context(
                    &input.invocation.session_id,
                    Some(&input.invocation.invocation_id),
                    10_000,
                    8 * 1024 * 1024,
                )
                .await?;
            let replay = model_attempt::prompt(
                inner,
                input,
                &source,
                ModelPurpose::Main,
                cancellation,
                continuation_base,
                &input.invocation.invocation_id,
            )
            .await?;
            let ids: Vec<_> = local_calls.iter().map(|call| call.id.as_str()).collect();
            lane.confirm(&replay, &ids, output.response_id.as_deref());
        }
        completed_step = true;
    }
    if cancellation.is_cancelled() {
        Err(RunError::Cancelled)
    } else {
        Err(RunError::StepLimit)
    }
}
