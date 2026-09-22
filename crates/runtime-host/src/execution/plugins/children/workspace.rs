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

use super::{BoundCommands, CreateChild, Error, SessionConfiguration, storage};
use maka_fs_tools::worktree::Binding;
use maka_plugins::execution::ChildWorkspace;
use maka_runtime::execution::SandboxMode;
use sha2::{Digest, Sha256};

impl BoundCommands {
    pub(super) async fn plan_child_workspace(
        &self,
        host: &super::super::Executions,
        parent: &SessionConfiguration,
        request: &CreateChild,
        id: &str,
        fingerprint: &str,
    ) -> Result<Option<Binding>, Error> {
        if request.workspace != Some(ChildWorkspace::IsolatedGit) {
            return Ok(None);
        }
        if parent.sandbox_mode == SandboxMode::ReadOnly {
            return Err(Error::Denied);
        }
        // Creation replay must survive a now-dirty source or a moved child HEAD.
        if host
            .log
            .probe_session_create::<SessionConfiguration>(id, fingerprint)
            .await
            .map_err(storage)?
            .is_some()
        {
            return Ok(None);
        }
        if self.submission_stop.is_cancelled() {
            return Err(Error::Revoked);
        }
        let identity = serde_json::to_vec(&("worktree-v1", &self.root_id, id))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        host.plan_worktree(
            parent.workspace.host_cwd.clone(),
            format!("{:x}", Sha256::digest(identity)),
        )
        .await
        .map(Some)
        .map_err(|e| Error::Host(e.to_string()))
    }
}

pub(super) fn bind(
    child: &mut SessionConfiguration,
    workspace: Option<ChildWorkspace>,
    binding: Option<Binding>,
) -> Result<(), Error> {
    if workspace != Some(ChildWorkspace::IsolatedGit) {
        return Ok(());
    }
    let binding = binding.ok_or(Error::Conflict)?;
    let cwd = binding
        .directory()
        .to_str()
        .ok_or_else(|| Error::Invalid("worktree path is not UTF-8".into()))?
        .to_owned();
    child.workspace = maka_protocol::session::WorkspaceProjection {
        target: maka_protocol::session::WorkspaceTarget::HostPath { path: cwd.clone() },
        host_cwd: cwd,
    };
    child.worktree = Some(binding);
    child.workspace_origin = maka_runtime::execution::WorkspaceOrigin::Allocated;
    Ok(())
}

pub(super) fn matches_parent(
    child: &SessionConfiguration,
    parent: &SessionConfiguration,
    workspace: Option<ChildWorkspace>,
) -> bool {
    if workspace == Some(ChildWorkspace::IsolatedGit) {
        child.workspace_origin == maka_runtime::execution::WorkspaceOrigin::Allocated
            && child.worktree.as_ref().is_some_and(|binding| {
                binding.source() == std::path::Path::new(&parent.workspace.host_cwd)
                    && binding.directory() == std::path::Path::new(&child.workspace.host_cwd)
            })
    } else {
        child.workspace.host_cwd == parent.workspace.host_cwd
            && child.workspace_origin == parent.workspace_origin
    }
}
