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

mod auto_context;
mod compact;
mod continuation;
mod history;
mod model_attempt;
pub use history::project as project_model_history;
mod prune;
pub mod recovery;
mod runner;
mod running;
mod steps;
pub use running::RunningInvocation;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use maka_event_log::{EventLog, StoreError};
use maka_js_runtime::CodeExecutor;
use maka_model::{ModelError, ModelExecutor, ProviderConfig};
use maka_runtime::event::{CommitError, Invocation};
use maka_runtime::execution::InvocationConfiguration;
use maka_runtime::input::MessageInput;
use maka_runtime::tools::ToolError;
use maka_tools::ToolCatalog;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("session already has an active invocation")]
    Busy,
    #[error("session requires reconciliation before another invocation: {0}")]
    ReconciliationRequired(String),
    #[error("invalid execution input: {0}")]
    InvalidInput(String),
    #[error("execution cancelled")]
    Cancelled,
    #[error("model step limit reached")]
    StepLimit,
    #[error(transparent)]
    Commit(#[from] CommitError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error("execution worker failed: {0}")]
    Internal(String),
}

pub struct RunInput {
    pub invocation: Invocation,
    pub context: Option<maka_runtime::context::ModelRequestContext>,
    pub work: RunWork,
    /// Stable domain admission identity, when this invocation was client-issued.
    pub request_fingerprint: Option<String>,
    pub provider: ProviderConfig,
    pub provider_options: Value,
    /// Frozen Main-only SDK output limit. Summary retains its own fixed limit.
    pub main_output_limit: Option<u64>,
    /// Frozen selected-model capability; image bytes are projected per request.
    pub supports_vision: bool,
    pub configuration: InvocationConfiguration,
}

pub enum RunWork {
    Continuation {
        source: maka_runtime::continuation::RunBoundary,
        tools: ToolCatalog,
        max_steps: usize,
    },
    Message {
        message: MessageInput,
        source_messages: Vec<maka_runtime::message::RootSourceMessage>,
        skill_invocation: Option<Box<maka_runtime::skills::SkillInvocationResult>>,
        tools: ToolCatalog,
        max_steps: usize,
    },
    ContextCompact,
}

struct Inner {
    log: Arc<EventLog>,
    model: ModelExecutor,
    cells: CodeExecutor,
    active: Mutex<HashSet<String>>,
    workers: TaskTracker,
}

/// Owns session admission and the entire invocation lifetime, including effects
/// still draining after the caller stops waiting.
#[derive(Clone)]
pub struct Engine(Arc<Inner>);

impl Engine {
    pub fn new(log: Arc<EventLog>, model: ModelExecutor, cells: CodeExecutor) -> Self {
        Self(Arc::new(Inner {
            log,
            model,
            cells,
            active: Mutex::new(HashSet::new()),
            workers: TaskTracker::new(),
        }))
    }

    pub async fn run(
        &self,
        input: RunInput,
        cancellation: CancellationToken,
    ) -> Result<Invocation, RunError> {
        self.start(input, cancellation).await?.wait().await
    }

    /// The owner first stops admissions and requests cancellation. This barrier
    /// also covers workers whose admission waiter was dropped before receiving
    /// a RunningInvocation handle.
    pub async fn drain(&self) {
        self.0.workers.close();
        self.0.workers.wait().await;
    }

    /// Returns only after durable admission. The returned owner covers execution
    /// and draining; dropping it requests cancellation without abandoning effects.
    pub async fn start(
        &self,
        input: RunInput,
        cancellation: CancellationToken,
    ) -> Result<RunningInvocation, RunError> {
        if input
            .main_output_limit
            .is_some_and(|limit| limit == 0 || limit > 10_000_000_000)
        {
            return Err(RunError::InvalidInput("invalid Main output limit".into()));
        }
        if let Some(context) = &input.context {
            context
                .validate()
                .map_err(|reason| RunError::InvalidInput(reason.into()))?;
        }
        match &input.work {
            RunWork::Message {
                message, max_steps, ..
            } if *max_steps == 0 || *max_steps > 256 || message.text_bytes() > 64 * 1024 => {
                return Err(RunError::InvalidInput("step or message limit".into()));
            }
            RunWork::Continuation {
                source, max_steps, ..
            } => {
                if *max_steps == 0
                    || *max_steps > 256
                    || source.invocation.session_id != input.invocation.session_id
                    || input
                        .request_fingerprint
                        .as_ref()
                        .is_none_or(|s| !maka_runtime::archive::valid_projection_digest(s))
                {
                    return Err(RunError::InvalidInput(
                        "invalid continuation request".into(),
                    ));
                }
            }
            RunWork::ContextCompact
                if input
                    .request_fingerprint
                    .as_ref()
                    .is_none_or(|value| value.is_empty()) =>
            {
                return Err(RunError::InvalidInput(
                    "compaction requires request fingerprint".into(),
                ));
            }
            RunWork::Message {
                message,
                source_messages,
                skill_invocation,
                ..
            } => {
                maka_runtime::message::validate_opening(
                    message,
                    source_messages,
                    skill_invocation.as_deref(),
                )
                .map_err(|reason| RunError::InvalidInput(reason.into()))?;
            }
            _ => {}
        }
        if !std::path::Path::new(&input.configuration.cwd).is_absolute()
            || input.configuration.cwd.len() > 4096
            || input.configuration.cwd.contains('\0')
            || input
                .configuration
                .model
                .as_ref()
                .is_some_and(|binding| binding.model != input.provider.model)
        {
            return Err(RunError::InvalidInput(
                "invalid invocation configuration".into(),
            ));
        }
        let session_id = input.invocation.session_id.clone();
        if !self
            .0
            .active
            .lock()
            .map_err(|_| RunError::Internal("admission poisoned".into()))?
            .insert(session_id.clone())
        {
            return Err(RunError::Busy);
        }
        let admission = Admission {
            inner: self.0.clone(),
            session_id,
        };
        let cancellation = cancellation.child_token();
        let cancel_on_drop = cancellation.clone().drop_guard();
        let inner = self.0.clone();
        let invocation = input.invocation.clone();
        let tool_names = Arc::new(match &input.work {
            RunWork::Message { tools, .. } | RunWork::Continuation { tools, .. } => {
                tools.names().into_iter().collect()
            }
            RunWork::ContextCompact => Default::default(),
        });
        let worker_cancellation = cancellation.clone();
        let (admitted, ready) = tokio::sync::oneshot::channel();
        let worker = self.0.workers.spawn(async move {
            let _admission = admission;
            runner::run(inner, input, worker_cancellation, admitted).await
        });
        if ready.await.is_err() {
            return match worker.await {
                Ok(Err(error)) => Err(error),
                Ok(Ok(_)) => Err(RunError::Internal(
                    "worker omitted durable admission".into(),
                )),
                Err(error) => Err(RunError::Internal(error.to_string())),
            };
        }
        cancel_on_drop.disarm();
        Ok(RunningInvocation::new(
            invocation,
            tool_names,
            cancellation,
            worker,
        ))
    }
}

struct Admission {
    inner: Arc<Inner>,
    session_id: String,
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.inner.active.lock().unwrap().remove(&self.session_id);
    }
}
