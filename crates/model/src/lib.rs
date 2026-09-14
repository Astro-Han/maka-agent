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

pub mod connection;
mod delta;
mod events;
pub mod oauth;
#[cfg(test)]
mod overflow_tests;
pub mod prompt;
mod responses;
mod step;
pub use maka_runtime::{model::ModelEvent, tools::ToolDefinition};
pub use responses::ResponsesLane;
pub use step::StepBuilder;

use maka_js_runtime::trusted::{ProviderEvent, TrustedError, TrustedRuntime};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

mod auth;
pub use auth::{AuthResolver, ProviderAuth};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenaiChat,
    OpenaiResponses,
    OpenaiCompatible { name: String },
    Anthropic,
}

/// Trusted configuration, never exposed to a Code Mode isolate or event log.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(skip)]
    pub network: maka_network::Policy,
    pub kind: ProviderKind,
    pub model: String,
    pub base_url: String,
    #[serde(flatten)]
    pub auth: ProviderAuth,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_overlay: Option<serde_json::Map<String, Value>>,
}

/// SDK-boundary values only. The agent layer must project canonical facts into
/// this request; neither these values nor SDK state are a history authority.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequest {
    pub provider: ProviderConfig,
    pub prompt: Vec<prompt::Message>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_tools"
    )]
    pub tools: Vec<ToolDefinition>,
    pub provider_options: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

fn serialize_tools<S: serde::Serializer>(
    tools: &[ToolDefinition],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum SdkTool<'a> {
        Function(&'a ToolDefinition),
    }
    serializer.collect_seq(tools.iter().map(SdkTool::Function))
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ModelError {
    #[error("model request cancelled")]
    Cancelled,
    #[error("model request deadline exceeded")]
    TimedOut,
    #[error("model input exceeds provider capacity (observed output: {observed_output})")]
    ContextOverflow { observed_output: bool },
    #[error("model adapter failed: {0}")]
    Adapter(String),
}

/// Bounded stream of SDK events. SDK-specific normalization belongs at this
/// crate's boundary, not in storage or client protocol.
pub struct ModelStream {
    receiver: mpsc::Receiver<Result<ProviderEvent, TrustedError>>,
    cancellation: CancellationToken,
    worker: tokio::task::JoinHandle<()>,
    normalizer: events::Normalizer,
    pending_delta: Option<delta::PendingDelta>,
    ended: bool,
}

impl ModelStream {
    pub async fn next(&mut self) -> Option<Result<ModelEvent, ModelError>> {
        if self.ended {
            return None;
        }
        loop {
            if self.cancellation.is_cancelled() {
                self.pending_delta = None;
            } else if let Some(event) = self.pending_delta.as_mut().and_then(Iterator::next) {
                return Some(Ok(event));
            } else {
                self.pending_delta = None;
            }
            let result = match self.receiver.recv().await {
                // Preserve the worker's cancellation/deadline cause, but do not
                // keep journaling buffered output after cancellation.
                Some(Ok(_)) if self.cancellation.is_cancelled() => continue,
                Some(Ok(value)) => self.normalizer.push(value.into_value()),
                Some(Err(error)) => Err(error.into()),
                None => {
                    self.ended = true;
                    if self.cancellation.is_cancelled() {
                        return Some(Err(ModelError::Cancelled));
                    }
                    return self.normalizer.end().err().map(Err);
                }
            };
            match result {
                Ok(None) => continue,
                Ok(Some(ModelEvent::PartDelta {
                    id,
                    text,
                    provider_options,
                })) if text.len() > 8 * 1024 => {
                    self.pending_delta = Some(delta::PendingDelta::new(id, text, provider_options));
                }
                Ok(Some(event)) => return Some(Ok(event)),
                Err(error) => {
                    self.ended = true;
                    self.cancellation.cancel();
                    self.receiver.close();
                    return Some(Err(error));
                }
            }
        }
    }

    pub async fn cancel_and_wait(mut self) {
        self.cancellation.cancel();
        self.receiver.close();
        let _ = (&mut self.worker).await;
    }
}

impl Drop for ModelStream {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
pub struct ModelExecutor {
    permits: Arc<Semaphore>,
    deadline: Duration,
    runtime: TrustedRuntime,
}

impl ModelExecutor {
    pub fn new(concurrency: usize, deadline: Duration) -> Result<Self, ModelError> {
        Self::with_runtime(TrustedRuntime::default(), concurrency, deadline)
    }

    pub fn with_runtime(
        runtime: TrustedRuntime,
        concurrency: usize,
        deadline: Duration,
    ) -> Result<Self, ModelError> {
        if concurrency == 0 || deadline.is_zero() {
            return Err(ModelError::Adapter("invalid model execution limits".into()));
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(concurrency)),
            deadline,
            runtime,
        })
    }

    pub async fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        self.stream_in_lane(request, cancellation, None).await
    }

    pub async fn stream_in_lane(
        &self,
        mut request: ModelRequest,
        cancellation: CancellationToken,
        lane: Option<ResponsesLane>,
    ) -> Result<ModelStream, ModelError> {
        // This request crosses JSON.parse into the SDK. Root pins impose their
        // own narrower policy; the generic boundary must not round JS numbers.
        if request
            .max_output_tokens
            .is_some_and(|value| value == 0 || value > 9_007_199_254_740_991)
        {
            return Err(ModelError::Adapter(
                "maxOutputTokens must be a positive JavaScript safe integer".into(),
            ));
        }
        let cancellation = cancellation.child_token();
        let permit = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(ModelError::Cancelled),
            permit = self.permits.clone().acquire_owned() =>
                permit.map_err(|error| ModelError::Adapter(error.to_string()))?,
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(ModelError::Cancelled),
            result = auth::resolve(&mut request.provider.auth) => result?,
        }
        auth::prepare(&mut request)?;
        let (sender, receiver) = mpsc::channel(32);
        let worker_cancel = cancellation.clone();
        let deadline = self.deadline;
        let runtime = self.runtime.clone();
        let network = request.provider.network.clone();
        if let Some(lane) = &lane {
            lane.prepare(&mut request);
        }
        let lane = lane.map(|lane| lane.transport);
        let request = serde_json::to_value(request)
            .map_err(|error| ModelError::Adapter(error.to_string()))?;
        let worker = tokio::spawn(async move {
            let _permit = permit;
            let failure_sender = sender.clone();
            let result = runtime
                .model_in_lane(request, sender, worker_cancel, deadline, lane, network)
                .await;
            if let Err(error) = result {
                let _ = failure_sender.send(Err(error)).await;
            }
        });
        Ok(ModelStream {
            receiver,
            cancellation,
            worker,
            normalizer: events::Normalizer::default(),
            pending_delta: None,
            ended: false,
        })
    }
}

impl From<TrustedError> for ModelError {
    fn from(error: TrustedError) -> Self {
        match error {
            TrustedError::Cancelled => Self::Cancelled,
            TrustedError::TimedOut => Self::TimedOut,
            TrustedError::Failed(message) => Self::Adapter(message),
        }
    }
}
