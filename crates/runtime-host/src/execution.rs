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

//! Owns live execution lifetimes; durable facts remain the query authority.
mod admission;
mod archive;
mod compact;
mod control;
mod handoff;
pub(crate) use handoff::CooperativeRun;
pub(crate) mod input;
mod interrupt;
mod launch;
pub(crate) mod message;
mod plugins;
pub(crate) use plugins::ResourceTarget;
mod prepare;
mod provider;
mod read;
mod recovery;
mod resume;
mod shell;
pub(crate) mod snapshot;
mod successor;
mod tools;
mod workspaces;

use crate::server::capabilities::Capabilities;
use maka_agent::{Engine, RunError};
use maka_config::ConfigurationStore;
use maka_event_log::{EventLog, StoreError};
use maka_js_runtime::{CellLimits, CodeExecutor};
use maka_model::ModelExecutor;
use maka_protocol::turn::*;
use maka_protocol::{OperationError, OperationErrorCode as Code};
use maka_runtime::tools::ToolError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

type Result<T> = std::result::Result<T, OperationError>;

pub(crate) struct Executions {
    pub(crate) plugin_catalog: maka_plugins::contributions::Catalog,
    pub(crate) plugin_calls: maka_plugins::call::Issuer,
    pub(crate) shells: Arc<crate::shell::ShellResources>,
    pub(crate) controllers: crate::controllers::Controllers,
    catalog: Arc<crate::server::CatalogFeed>,
    engine: Engine,
    models: ModelExecutor,
    log: Arc<EventLog>,
    configuration: Arc<ConfigurationStore>,
    // Control-plane discovery shares this owner; requests drain before workers.
    pub(crate) oauth: crate::oauth::Authority,
    paths: ExecutionPaths,
    writes: Arc<maka_fs_tools::WriteCoordinator>,
    capabilities: Arc<Capabilities>,
    // Shared admission gate covers execution/control/interaction publication,
    // never the model, effect, or human approval lifetime.
    interactions: Arc<crate::server::interactions::Interactions>,
    active: Mutex<HashMap<String, ActiveRun>>,
    plugin_processes: Arc<Mutex<HashMap<String, usize>>>,
    workers: TaskTracker,
    handoff_wake: tokio::sync::Notify,
    shutdown: CancellationToken,
}

#[derive(Clone)]
struct ActiveRun {
    invocation: maka_runtime::event::Invocation,
    tool_names: Arc<std::collections::HashSet<String>>,
    cancellation: CancellationToken,
    completed: CancellationToken,
    handoff: Option<maka_agent::HandoffGate>,
}

pub(crate) struct ExecutionPaths {
    pub state_root: std::path::PathBuf,
}

impl Executions {
    pub(crate) fn new(
        log: Arc<EventLog>,
        configuration: Arc<ConfigurationStore>,
        shutdown: CancellationToken,
        capabilities: Arc<Capabilities>,
        interactions: Arc<crate::server::interactions::Interactions>,
        paths: ExecutionPaths,
        runtime: maka_js_runtime::trusted::TrustedRuntime,
    ) -> std::result::Result<Self, crate::server::HostError> {
        let workers = TaskTracker::new();
        let plugin_calls = maka_plugins::call::Issuer::default();
        let plugin_catalog = maka_plugins::contributions::Catalog::with_calls(plugin_calls.clone());
        tools::reserve_core_names(&plugin_catalog)?;
        let models = ModelExecutor::with_runtime(runtime.clone(), 64, Duration::from_secs(120))?;
        Ok(Self {
            plugin_catalog,
            plugin_calls,
            oauth: crate::oauth::Authority::new(workers.clone(), shutdown.clone()),
            controllers: Default::default(),
            shells: Arc::new(crate::shell::ShellResources::with_runtime(
                log.clone(),
                shutdown.clone(),
                runtime.clone(),
            )),
            engine: Engine::new(
                log.clone(),
                models.clone(),
                CodeExecutor::new(4, CellLimits::default())?,
            ),
            log,
            models,
            configuration,
            paths,
            capabilities,
            catalog: interactions.catalog.clone(),
            interactions,
            writes: Arc::new(maka_fs_tools::WriteCoordinator::default()),
            active: Mutex::new(HashMap::new()),
            plugin_processes: Arc::default(),
            workers,
            handoff_wake: tokio::sync::Notify::new(),
            shutdown,
        })
    }

    pub(crate) fn active_count(&self) -> usize {
        self.active.lock().unwrap().len()
    }
    pub(crate) fn accepting(&self) -> bool {
        !self.shutdown.is_cancelled()
            && *self
                .interactions
                .retirement
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                == crate::server::retirement::Phase::Ready
    }
    /// Existing admitted commands may complete derived work while the Host
    /// prepares a handoff. Fresh queue/recovery admissions still require Ready.
    pub(crate) fn retiring(&self) -> bool {
        self.shutdown.is_cancelled()
            || *self
                .interactions
                .retirement
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                == crate::server::retirement::Phase::Retiring
    }
    pub(crate) fn has_active_session(&self, session: &str) -> bool {
        self.active
            .lock()
            .unwrap()
            .values()
            .any(|run| run.invocation.session_id == session)
    }

    pub(crate) async fn lock_admission(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.interactions.lock_admission().await
    }

    /// Caller holds the admission gate: terminal facts do not release cleanup
    /// ownership, and accepted messages are work even before their next root.
    pub(crate) async fn has_session_work(
        &self,
        session: &str,
    ) -> std::result::Result<bool, StoreError> {
        Ok(self.has_active_session(session)
            || !self.log.pending_messages(session).await?.is_empty())
    }

    pub(crate) async fn shutdown(&self) {
        self.begin_drain();
        self.workers.close();
        self.workers.wait().await;
        self.engine.drain().await;
    }

    pub(crate) fn begin_drain(&self) {
        self.shutdown.cancel();
    }

    async fn recorded(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<snapshot::RecordedTurn>> {
        Ok(self
            .log
            .turn_boundary(session_id, turn_id)
            .await
            .map_err(internal)?
            .map(snapshot::project))
    }

    pub(crate) async fn query(&self, input: TurnQueryInput) -> Result<TurnSnapshot> {
        self.recorded(&input.session_id, &input.turn_id)
            .await?
            .map(|record| record.snapshot)
            .ok_or_else(|| failure(Code::NotFound, "Turn does not exist"))
    }

    pub(crate) async fn stop(self: &Arc<Self>, input: TurnStopInput) -> Result<TurnSnapshot> {
        let mut admission = Some(self.lock_admission().await);
        let boundary = self
            .log
            .turn_boundary(&input.session_id, &input.turn_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::NotFound, "Turn does not exist"))?;
        if boundary.root_invocation().run_id != input.run_id {
            return Err(failure(Code::OperationConflict, "Run identity changed"));
        }
        let paused = matches!(
            boundary.state,
            maka_event_log::turns::InvocationState::Ended {
                outcome: maka_runtime::event::InvocationOutcome::HandoffPaused { .. },
                ..
            }
        );
        let boundary = if paused {
            self.log
                .cancel_handoff(&boundary.invocation)
                .await
                .map_err(|error| match error {
                    StoreError::CommitUnknown(_) | StoreError::OperationUnknown => {
                        self.begin_drain();
                        // turn.stop has no commit_outcome_unknown wire outcome.
                        // Fence admission without inventing a client protocol code.
                        failure(Code::HostDraining, &error.to_string())
                    }
                    _ => internal(error),
                })?
        } else {
            boundary
        };
        let invocation = boundary.invocation;
        let cancellation = self
            .active
            .lock()
            .unwrap()
            .get(&invocation.run_id)
            .map(|run| run.cancellation.clone());
        if let Some(cancellation) = cancellation {
            self.interactions.stop_run(&invocation).await?;
            cancellation.cancel();
        }
        if paused {
            self.dispatch_pending(&input.session_id, &mut admission)
                .await?;
        }
        self.query(TurnQueryInput {
            session_id: input.session_id,
            turn_id: input.turn_id,
        })
        .await
    }
}

// Runtime-generated facts that cannot be committed, or effects without a
// durable outcome, invalidate continued admission. Ordinary model/tool errors
// and bounded-history rejections do not invalidate the host.
fn requires_drain(error: &RunError) -> bool {
    matches!(
        error,
        RunError::Commit(_)
            | RunError::Store(StoreError::CommitUnknown(_) | StoreError::OperationUnknown)
            | RunError::Tool(ToolError::Persistence(_) | ToolError::CleanupUnconfirmed(_))
    )
}

fn execution_error(error: RunError) -> OperationError {
    let code = match &error {
        RunError::Commit(maka_runtime::event::CommitError::OutcomeUnknown(_))
        | RunError::Store(StoreError::CommitUnknown(_) | StoreError::OperationUnknown)
        | RunError::Tool(ToolError::Persistence(_) | ToolError::CleanupUnconfirmed(_)) => {
            Code::OutcomeUnknown
        }
        RunError::Busy => Code::SessionBusy,
        RunError::ReconciliationRequired(_) => Code::OperationUnavailable,
        RunError::InvalidInput(_) => Code::OperationUnavailable,
        _ => Code::InternalFailure,
    };
    failure(code, &error.to_string())
}
impl Executions {
    async fn ordinary_session(&self, session: &str) -> Result<()> {
        crate::session::require_unmanaged(&self.log, session, Code::OperationConflict).await
    }
}

fn internal(error: impl std::fmt::Display) -> OperationError {
    failure(Code::InternalFailure, &error.to_string())
}
fn failure(code: Code, message: &str) -> OperationError {
    OperationError {
        code,
        message: message.chars().take(1024).collect(),
    }
}
