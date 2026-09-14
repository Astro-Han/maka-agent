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
mod interrupt;
mod launch;
mod message;
mod prepare;
mod prompt;
mod provider;
mod read;
mod recovery;
mod resume;
mod shell;
pub(crate) mod skills;
pub(crate) mod snapshot;
mod successor;
mod tools;

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
    pub(crate) shells: Arc<crate::shell::ShellResources>,
    pub(crate) controllers: crate::controllers::Controllers,
    engine: Engine,
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
    workers: TaskTracker,
    shutdown: CancellationToken,
}

#[derive(Clone)]
struct ActiveRun {
    invocation: maka_runtime::event::Invocation,
    tool_names: Arc<std::collections::HashSet<String>>,
    cancellation: CancellationToken,
    completed: CancellationToken,
}

pub(crate) struct ExecutionPaths {
    pub state_root: std::path::PathBuf,
    pub global_instructions: Option<std::path::PathBuf>,
    pub skill_home: Option<std::path::PathBuf>,
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
        Ok(Self {
            oauth: crate::oauth::Authority::new(workers.clone(), shutdown.clone()),
            controllers: Default::default(),
            shells: Arc::new(crate::shell::ShellResources::with_runtime(
                log.clone(),
                shutdown.clone(),
                runtime.clone(),
            )),
            engine: Engine::new(
                log.clone(),
                ModelExecutor::with_runtime(runtime, 64, Duration::from_secs(120))?,
                CodeExecutor::new(4, CellLimits::default())?,
            ),
            log,
            configuration,
            paths,
            capabilities,
            interactions,
            writes: Arc::new(maka_fs_tools::WriteCoordinator::default()),
            active: Mutex::new(HashMap::new()),
            workers,
            shutdown,
        })
    }

    pub(crate) fn active_count(&self) -> usize {
        self.active.lock().unwrap().len()
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

    pub(crate) async fn stop(&self, input: TurnStopInput) -> Result<TurnSnapshot> {
        let _admission = self.lock_admission().await;
        let boundary = self
            .log
            .turn_boundary(&input.session_id, &input.turn_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| failure(Code::NotFound, "Turn does not exist"))?;
        let invocation = boundary.invocation.clone();
        let snapshot = snapshot::project(boundary).snapshot;
        if snapshot.run_id != input.run_id {
            return Err(failure(Code::OperationConflict, "Run identity changed"));
        }
        let cancellation = self
            .active
            .lock()
            .unwrap()
            .get(&input.run_id)
            .map(|run| run.cancellation.clone());
        if let Some(cancellation) = cancellation {
            self.interactions.stop_run(&invocation).await?;
            cancellation.cancel();
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
            | RunError::Tool(ToolError::Persistence(_) | ToolError::OutcomeUnknown(_))
    )
}

fn execution_error(error: RunError) -> OperationError {
    let code = match &error {
        RunError::Busy => Code::SessionBusy,
        RunError::ReconciliationRequired(_) => Code::OperationUnavailable,
        RunError::InvalidInput(_) => Code::OperationUnavailable,
        _ => Code::InternalFailure,
    };
    failure(code, &error.to_string())
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
