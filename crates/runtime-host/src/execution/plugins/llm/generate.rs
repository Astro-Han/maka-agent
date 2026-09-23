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

use maka_event_log::{
    EventLog,
    usage::{AuxiliarySource, Outcome},
};
use maka_model::{ModelExecutor, ModelRequest, ModelStream, StepBuilder};
use maka_runtime::{
    model::{ModelEvent, ModelGeneration, ModelPart, ModelStep, TextKind},
    tools::ToolError,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) async fn generate(
    models: ModelExecutor,
    request: ModelRequest,
    adapter: maka_plugins::model::Binding,
    cancellation: CancellationToken,
    log: Arc<EventLog>,
    source: AuxiliarySource,
    quote: maka_runtime::pricing::Quote,
) -> Result<ModelGeneration, ToolError> {
    if cancellation.is_cancelled() {
        return Err(failed("model cancelled before dispatch"));
    }
    let id = log
        .begin_auxiliary_model(source, Some(quote))
        .await
        .map_err(persistence)?;
    let model_id = request.provider.model.clone();
    let result = async {
        let stream = models
            .stream_with_adapter(request, cancellation.clone(), None, adapter)
            .await
            .map_err(failed)?;
        let step = receive(stream, &log, id).await?;
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
        Ok(ModelGeneration {
            text,
            model_id,
            finish_reason: step.finish_reason,
            usage: step.usage,
        })
    }
    .await;
    // A failed fact commit may have committed. Do not overwrite that uncertainty
    // or let an auxiliary failure release the Host's persistence fence.
    if matches!(result, Err(ToolError::Persistence(_))) {
        return result;
    }
    let outcome = match &result {
        Ok(_) => Outcome::Success,
        Err(_) if cancellation.is_cancelled() => Outcome::Aborted,
        Err(_) => Outcome::Error,
    };
    log.settle_auxiliary_model(id, outcome)
        .await
        .map_err(persistence)?;
    result
}

async fn receive(
    mut stream: ModelStream,
    log: &EventLog,
    id: Uuid,
) -> Result<ModelStep, ToolError> {
    let result = async {
        let mut builder = StepBuilder::default();
        let mut bytes = 0usize;
        let mut observed_usage = false;
        while let Some(event) = stream.next().await {
            let event = event.map_err(failed)?;
            if let ModelEvent::Finished { usage, .. } = &event
                && !observed_usage
            {
                log.observe_auxiliary_model(id, usage.clone())
                    .await
                    .map_err(persistence)?;
                observed_usage = true;
            }
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
    result
}

fn persistence(error: impl ToString) -> ToolError {
    ToolError::Persistence(error.to_string())
}
fn failed(error: impl ToString) -> ToolError {
    ToolError::Failed(error.to_string())
}
