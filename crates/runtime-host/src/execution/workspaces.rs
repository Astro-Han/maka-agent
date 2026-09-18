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

use super::{Executions, failure, internal};
use crate::session::SessionConfiguration;
use maka_fs_tools::worktree::{Binding, Worktrees};
use maka_protocol::OperationErrorCode as Code;
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

impl Executions {
    pub(crate) fn own_plugin_process(&self, session: &str) -> ProcessActivity {
        *self
            .plugin_processes
            .lock()
            .unwrap()
            .entry(session.into())
            .or_default() += 1;
        ProcessActivity {
            registry: self.plugin_processes.clone(),
            session: session.into(),
        }
    }
    pub(super) async fn prepare_worktree(
        &self,
        session: &SessionConfiguration,
    ) -> super::Result<()> {
        let Some(binding) = session.worktree.clone() else {
            return Ok(());
        };
        if binding.directory() != std::path::Path::new(&session.workspace.host_cwd) {
            return Err(failure(
                Code::OperationConflict,
                "Session workspace differs from its worktree binding",
            ));
        }
        let root = self.paths.state_root.join("subagent-worktrees");
        blocking(self.shutdown.clone(), move |cancel| {
            Worktrees::open(&root)?.ensure(&binding, &cancel)
        })
        .await
        .map_err(internal)
    }
    pub(super) async fn plan_worktree(&self, source: String, id: String) -> io::Result<Binding> {
        let root = self.paths.state_root.join("subagent-worktrees");
        blocking(self.shutdown.clone(), move |cancel| {
            Worktrees::open(&root)?.plan(std::path::Path::new(&source), &id, cancel)
        })
        .await
    }
}

/// Instance-lifetime plugin processes outlive Turns but still write their Session workspace.
pub(crate) struct ProcessActivity {
    registry: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
    session: String,
}
impl Drop for ProcessActivity {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap();
        let count = registry.get_mut(&self.session).expect("registered process");
        *count -= 1;
        if *count == 0 {
            registry.remove(&self.session);
        }
    }
}

/// Cancellation also propagates when the preparing future itself is dropped.
struct Interrupt(Arc<AtomicBool>);
impl Drop for Interrupt {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
pub(super) async fn blocking<T: Send + 'static>(
    shutdown: CancellationToken,
    run: impl FnOnce(Arc<AtomicBool>) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let interrupt = Interrupt(Arc::new(AtomicBool::new(false)));
    let cancel = interrupt.0.clone();
    let task = tokio::task::spawn_blocking(move || run(cancel));
    tokio::select! {
        result = tokio::time::timeout(std::time::Duration::from_secs(60), task) => {
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "worktree preparation timed out"))?
                .map_err(io::Error::other)?
        }
        _ = shutdown.cancelled() => Err(io::Error::new(io::ErrorKind::Interrupted, "Host is shutting down")),
    }
}
