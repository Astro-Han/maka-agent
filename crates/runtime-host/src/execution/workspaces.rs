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
    /// Managed checkouts cannot acquire an untracked owner through a path-only
    /// selection. Shared children inherit the binding and its lifecycle instead.
    pub(crate) fn validate_workspace(&self, session: &SessionConfiguration) -> super::Result<()> {
        let root = self.paths.state_root.join("subagent-worktrees");
        let root = maka_fs_tools::workspace::project::host_path(&root).map_err(internal)?;
        let cwd = std::path::Path::new(&session.workspace.host_cwd);
        if session
            .worktree
            .as_ref()
            .is_some_and(|binding| binding.directory() != cwd)
            || (session.worktree.is_none() && cwd.starts_with(root))
        {
            return Err(failure(
                Code::OperationConflict,
                "Managed workspaces require their owning Session binding",
            ));
        }
        Ok(())
    }

    pub(super) async fn prepare_worktree(
        &self,
        session: &SessionConfiguration,
    ) -> super::Result<()> {
        self.validate_workspace(session)?;
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
