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

use crate::{
    Inner, RunError, RunInput, history,
    runner::{append, digest},
};
use maka_event_log::context::ModelContextSource;
use maka_model::prompt::Message;
use maka_model::{ModelRequest, StepBuilder, ToolDefinition};
use maka_runtime::{
    context::ModelPurpose,
    event::{Fact, ModelInterruption},
    model::ModelStep,
};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(super) enum Attempt {
    Main(maka_model::ResponsesLane),
    Summary,
}

pub(super) async fn prompt(
    inner: &Inner,
    input: &RunInput,
    source: &ModelContextSource,
    purpose: ModelPurpose,
    cancellation: &CancellationToken,
) -> Result<Vec<Message>, RunError> {
    let mut prompt = history::materialize(
        &inner.log,
        &source.tail,
        source.anchor.as_ref(),
        &input.invocation.session_id,
        input.supports_vision,
        cancellation,
    )
    .await?;
    if let Some(baseline) = &source.baseline {
        let text = match purpose {
            ModelPurpose::Summary => format!(
                "Previous continuation summary:\n{}\n\nUpdate it using the newer conversation events that follow.",
                baseline.checkpoint.summary.text
            ),
            ModelPurpose::Main => format!(
                "Continuation summary:\n{}",
                baseline.checkpoint.summary.text
            ),
        };
        prompt.insert(0, Message::user(text));
    }
    if purpose == ModelPurpose::Main
        && let Some(system) = &input.configuration.system_prompt
    {
        prompt.insert(
            0,
            Message::System {
                content: system.text.clone(),
                provider_options: None,
            },
        );
    }
    Ok(prompt)
}

pub(super) async fn execute(
    inner: &Arc<Inner>,
    input: &RunInput,
    source: &ModelContextSource,
    prompt: Vec<Message>,
    definitions: Vec<ToolDefinition>,
    attempt: Attempt,
    cancellation: &CancellationToken,
) -> Result<(String, ModelStep), RunError> {
    let (purpose, lane) = match attempt {
        Attempt::Main(lane) => (ModelPurpose::Main, Some(lane)),
        Attempt::Summary => (ModelPurpose::Summary, None),
    };
    if cancellation.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    let max_output_tokens = match purpose {
        ModelPurpose::Main => input.main_output_limit,
        ModelPurpose::Summary => Some(8000),
    };
    let prompt = if input.supports_vision
        && matches!(
            &input.provider.kind,
            maka_model::ProviderKind::OpenaiChat
                | maka_model::ProviderKind::OpenaiCompatible { .. }
        ) {
        history::project_chat(prompt)
    } else {
        prompt
    };
    let prompt = history::project_compatible(prompt, &input.provider.kind);
    let mut evidence = json!({"projection":"maka.model-history.v1","prompt":prompt,
        "tools":definitions,"providerOptions":input.provider_options});
    if let Some(limit) = max_output_tokens {
        evidence["maxOutputTokens"] = json!(limit);
    }
    let input_digest = digest(
        &serde_json::to_vec(&evidence).map_err(|error| RunError::Internal(error.to_string()))?,
    );
    let route_identity = digest(
        &serde_json::to_vec(&input.provider)
            .map_err(|error| RunError::Internal(error.to_string()))?,
    );
    let step_id = Uuid::new_v4().to_string();
    append(
        inner,
        &input.invocation,
        Fact::ModelRequested {
            effective_source_digest: (purpose == ModelPurpose::Summary
                || matches!(
                    source.source_evidence.scope,
                    maka_runtime::event::LogScope::Lineage { .. }
                ))
            .then(|| source.effective_source_digest.clone()),
            purpose: Some(purpose),
            context: input.context.clone(),
            step_id: step_id.clone(),
            model_id: input.provider.model.clone(),
            source_scope: source.source_evidence.scope.clone(),
            source_high_water: source.source_evidence.high_water,
            source_digest: source.source_evidence.digest.clone(),
            input_digest,
            route_identity,
            checkpoint_event_id: source
                .baseline
                .as_ref()
                .map(|baseline| baseline.event_id.clone()),
        },
    )
    .await?;
    let result: Result<_, RunError> = async {
        let mut stream = inner
            .model
            .stream_in_lane(
                ModelRequest {
                    provider: input.provider.clone(),
                    prompt,
                    tools: definitions,
                    provider_options: input.provider_options.clone(),
                    max_output_tokens,
                },
                cancellation.clone(),
                lane,
            )
            .await?;
        let mut builder = StepBuilder::for_step(&step_id)?;
        let result: Result<_, RunError> = async {
            while let Some(event) = stream.next().await {
                let event = event?;
                builder.push(event.clone())?;
                append(
                    inner,
                    &input.invocation,
                    Fact::ModelObserved {
                        step_id: step_id.clone(),
                        event,
                    },
                )
                .await?;
            }
            builder.finish().map_err(Into::into)
        }
        .await;
        stream.cancel_and_wait().await;
        let output = result?;
        if purpose == ModelPurpose::Main
            && output.finish_reason == maka_runtime::model::ModelFinishReason::Length
        {
            return Err(maka_model::ModelError::Adapter(
                "unsupported model finish reason: length".into(),
            )
            .into());
        }
        if purpose == ModelPurpose::Summary
            && output.parts.iter().any(|part| {
                matches!(
                    part,
                    maka_runtime::model::ModelPart::ToolCall { .. }
                        | maka_runtime::model::ModelPart::ToolResult { .. }
                )
            })
        {
            return Err(
                maka_model::ModelError::Adapter("summary contains tool content".into()).into(),
            );
        }
        Ok(output)
    }
    .await;
    let output = match result {
        Ok(output) => output,
        Err(RunError::Model(error)) => {
            let status = match error {
                maka_model::ModelError::Cancelled => ModelInterruption::Cancelled,
                maka_model::ModelError::TimedOut => ModelInterruption::TimedOut,
                maka_model::ModelError::Adapter(_) => ModelInterruption::Failed,
                maka_model::ModelError::ContextOverflow { .. } => ModelInterruption::Failed,
            };
            append(
                inner,
                &input.invocation,
                Fact::ModelInterrupted {
                    step_id: step_id.clone(),
                    status,
                },
            )
            .await?;
            return Err(error.into());
        }
        Err(error) => return Err(error),
    };
    append(
        inner,
        &input.invocation,
        Fact::ModelCompleted {
            step_id: step_id.clone(),
            output: output.clone(),
        },
    )
    .await?;
    if cancellation.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    Ok((step_id, output))
}
