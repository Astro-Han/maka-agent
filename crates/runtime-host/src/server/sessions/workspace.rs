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

use super::{Result, failure, invalid, mutation, stored};
use crate::{server::Host, session::SessionConfiguration};
use maka_event_log::sessions::SessionExecutionState;
use maka_protocol::{OperationErrorCode as Code, session::*};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

pub(super) async fn relocate(host: &Host, value: &Value) -> Result<SessionUpdateResult> {
    let input = decode_session_workspace_relocate_input(value).map_err(invalid)?;
    if input.session_id == "maka_workhub_coordination" {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub workspace requires WorkHub authority",
        ));
    }
    let _admission = host.executions.lock_admission().await;
    if host.draining.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let initial = host
        .log
        .get_session::<SessionConfiguration>(&input.session_id)
        .await
        .map_err(stored)?
        .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
    if initial.revision != input.expected_revision {
        return Ok(SessionUpdateResult::RevisionConflict {
            expected_revision: input.expected_revision,
            actual_revision: initial.revision,
        });
    }
    let workspace = resolve(host, &input.workspace).await?;
    if initial.configuration.worktree.is_some()
        && workspace.host_cwd != initial.configuration.workspace.host_cwd
    {
        return Err(failure(
            Code::OperationConflict,
            "An isolated child workspace cannot be relocated",
        ));
    }
    // Resolution may await I/O while an existing run or metadata mutation commits.
    // Relocation requires quiescence even when its canonical workspace is unchanged.
    let current = host
        .log
        .get_session::<SessionConfiguration>(&input.session_id)
        .await
        .map_err(stored)?
        .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
    if current
        .execution
        .as_ref()
        .is_some_and(|execution| matches!(execution.state, SessionExecutionState::Live { .. }))
        || host
            .executions
            .has_session_work(&input.session_id)
            .await
            .map_err(stored)?
    {
        return Err(failure(
            Code::SessionBusy,
            "Session workspace cannot change while a Turn is active",
        ));
    }
    if current.revision != input.expected_revision {
        return Ok(SessionUpdateResult::RevisionConflict {
            expected_revision: input.expected_revision,
            actual_revision: current.revision,
        });
    }
    if current.archived {
        return Err(failure(
            Code::OperationConflict,
            "Archived Session workspace cannot be relocated",
        ));
    }
    let committed = host
        .log
        .update_session_metadata(
            &input.session_id,
            input.expected_revision,
            move |configuration: &mut SessionConfiguration| {
                configuration.workspace = workspace;
                Ok(())
            },
        )
        .await
        .map_err(stored)?;
    let output = mutation::result(committed);
    assert_workspace_relocate_output_for_input(&input, &output).map_err(invalid)?;
    Ok(output)
}

pub(in crate::server) async fn resolve(
    host: &Host,
    target: &WorkspaceTarget,
) -> Result<WorkspaceProjection> {
    let path = match target {
        WorkspaceTarget::Project { project_id } => {
            return super::super::projects::resolve(host, project_id).await;
        }
        WorkspaceTarget::HostPath { path } => path.clone(),
    };
    let cwd = tokio::task::spawn_blocking(move || canonical_directory(&path))
        .await
        .map_err(|error| failure(Code::InternalFailure, &error.to_string()))??;
    Ok(WorkspaceProjection {
        target: WorkspaceTarget::HostPath { path: cwd.clone() },
        host_cwd: cwd,
    })
}

fn canonical_directory(path: &str) -> Result<String> {
    let invalid = || {
        failure(
            Code::InvalidRequest,
            "Workspace Host path is not an existing directory",
        )
    };
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(invalid());
    }
    // Node's realpath(resolve(path)) removes lexical parents before following
    // symlinks. canonicalize(path) alone follows symlinks first.
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    let canonical = normalized.canonicalize().map_err(|_| invalid())?;
    if !canonical.is_dir() {
        return Err(invalid());
    }
    // Use the same client/storage spelling as project-backed workspaces.
    // Windows canonicalize's verbatim prefix is an internal filesystem detail.
    maka_fs_tools::workspace::project::host_path(&canonical)
        .map(str::to_owned)
        .map_err(|_| invalid())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn canonical_workspace_preserves_lexical_resolution_before_following_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir_all(first.join("child")).unwrap();
        std::fs::create_dir_all(second.join("child")).unwrap();
        let link = first.join("alias");
        symlink(second.join("child"), &link).unwrap();
        assert_eq!(
            canonical_directory(link.to_str().unwrap()).unwrap(),
            second
                .join("child")
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
        );
        // Resolving alias/.. must select first, not second.
        assert_eq!(
            canonical_directory(link.join("..").to_str().unwrap()).unwrap(),
            first.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(canonical_directory("/../../").unwrap(), "/");
        let file = temp.path().join("file");
        std::fs::write(&file, "not a directory").unwrap();
        for invalid in [
            file.as_path(),
            Path::new("relative"),
            &temp.path().join("missing"),
        ] {
            assert_eq!(
                canonical_directory(invalid.to_str().unwrap())
                    .unwrap_err()
                    .code,
                Code::InvalidRequest
            );
        }
    }
}
