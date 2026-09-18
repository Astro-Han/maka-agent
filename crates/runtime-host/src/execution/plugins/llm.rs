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

use super::{Error, Executions, SessionConfiguration, storage};
use crate::execution::provider;
use maka_model::{ModelExecutor, ModelRequest, StepBuilder, prompt::Message};
use maka_plugins::{
    fiber::Context,
    llm::{Generate, ModelGeneration},
};
use maka_runtime::{
    event::Invocation,
    model::{ModelEvent, ModelPart, TextKind},
    tool_call::{ToolCallIdentity, ToolOrigin},
    tool_output::{ToolOutput, ToolSuccess},
    tools::{PreparedEffect, ToolError, ToolJournal},
};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

impl Executions {
    pub(crate) async fn plugin_model(
        self: &Arc<Self>,
        owner: Context,
        invocation: Invocation,
        parent_operation_id: Option<String>,
        input: Generate,
        cancellation: CancellationToken,
    ) -> Result<impl Future<Output = Result<Value, ToolError>> + Send + 'static, Error> {
        input
            .validate()
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let gate = self.interactions.own_admission().await;
        let lease = owner.admit().map_err(|_| Error::Revoked)?;
        let identity = owner.identity().map_err(|_| Error::Revoked)?;
        if !self.accepting() || cancellation.is_cancelled() {
            return Err(Error::Revoked);
        }
        let frozen = self
            .log
            .invocation_configuration(&invocation)
            .await
            .map_err(storage)?
            .ok_or(Error::Denied)?;
        let current = self
            .log
            .get_session::<SessionConfiguration>(&invocation.session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        if current.archived {
            return Err(Error::Denied);
        }
        let model = frozen
            .model
            .as_ref()
            .ok_or_else(|| Error::Invalid("invocation has no model binding".into()))?;
        let prepared = provider::observe_binding(
            &self.configuration,
            &invocation.session_id,
            model,
            frozen.thinking_level,
        )
        .await
        .map_err(|error| Error::Host(error.to_string()))?
        .admit(&self.oauth)
        .map_err(|error| Error::Host(error.to_string()))?;
        let evidence =
            serde_json::to_value(&input).map_err(|error| Error::Invalid(error.to_string()))?;
        let requested = input.max_output_tokens.unwrap_or(2048);
        let max_output_tokens = Some(
            prepared
                .main_output_limit
                .map_or(requested, |limit| limit.min(requested)),
        );
        let mut prompt = Vec::new();
        if let Some(content) = input.system {
            prompt.push(Message::System {
                content,
                provider_options: None,
            });
        }
        prompt.push(Message::user(input.prompt));
        let request = ModelRequest {
            provider: prepared.config,
            prompt,
            tools: Vec::new(),
            provider_options: prepared.options,
            max_output_tokens,
        };
        let models = self.models.clone();
        let effect = PreparedEffect::new(move |cancellation| {
            Box::pin(generate(models, request, cancellation))
        });
        let operation_id = uuid::Uuid::new_v4().to_string();
        let journal = ToolJournal::new(self.log.clone(), invocation);
        let host = self.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        self.workers.spawn(async move {
            drop(gate);
            let result = journal
                .invoke_prepared_call(
                    operation_id,
                    ToolCallIdentity {
                        tool_call_id: uuid::Uuid::new_v4().to_string(),
                        origin: ToolOrigin::HostSdk {
                            package_id: identity.package_id,
                            entry_id: identity.entry_id,
                            activation: identity.activation,
                            parent_operation_id,
                        },
                    },
                    "llm.generate".into(),
                    evidence,
                    cancellation,
                    effect,
                )
                .await;
            if matches!(
                result,
                Err(ToolError::Persistence(_) | ToolError::OutcomeUnknown(_))
            ) {
                host.begin_drain();
            }
            drop(lease);
            let _ = send.send(result);
        });
        Ok(async move {
            receive.await.map_err(|_| {
                ToolError::OutcomeUnknown("model resource worker disappeared".into())
            })?
        })
    }
}

async fn generate(
    models: ModelExecutor,
    request: ModelRequest,
    cancellation: CancellationToken,
) -> Result<ToolSuccess, ToolError> {
    let model_id = request.provider.model.clone();
    let mut stream = models.stream(request, cancellation).await.map_err(failed)?;
    let result = async {
        let mut builder = StepBuilder::default();
        let mut bytes = 0usize;
        while let Some(event) = stream.next().await {
            let event = event.map_err(failed)?;
            bytes = bytes.saturating_add(serde_json::to_vec(&event).map_err(failed)?.len());
            if bytes > 2 * 1024 * 1024 {
                return Err(failed("model generation exceeds 2 MiB stream limit"));
            }
            if matches!(
                event,
                ModelEvent::ToolCall(_) | ModelEvent::ProviderToolResult { .. }
            ) {
                return Err(failed("auxiliary model generation cannot call tools"));
            }
            builder.push(event).map_err(failed)?;
        }
        builder.finish().map_err(failed)
    }
    .await;
    // EOF alone does not release the shared worker's permits and transport.
    stream.cancel_and_wait().await;
    let step = result?;
    if step.finish_reason == maka_runtime::model::ModelFinishReason::ToolCalls {
        return Err(failed(
            "auxiliary model generation cannot finish with tool calls",
        ));
    }
    let text = step
        .parts
        .into_iter()
        .filter_map(|part| match part {
            ModelPart::Text {
                text_kind: TextKind::Text,
                text,
                ..
            } => Some(text),
            _ => None,
        })
        .collect();
    Ok(ToolOutput::Model(Box::new(ModelGeneration {
        text,
        model_id,
        finish_reason: step.finish_reason,
        usage: step.usage,
    }))
    .into())
}
fn failed(error: impl ToString) -> ToolError {
    ToolError::Failed(error.to_string())
}
