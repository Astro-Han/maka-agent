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

use super::{Code, Result, WorkspaceProjection, WorkspaceTarget, failure};
use maka_plugins::fiber::Context;
use std::{io, path::PathBuf};

pub(super) async fn prepare(owner: &Context, path: PathBuf) -> Result<WorkspaceProjection> {
    let pending = owner
        .spawn_resource("WorkHub workspace preparation", move |_| async move {
            tokio::task::spawn_blocking(move || {
                match std::fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(failure(Code::PersistenceFailed, error.to_string())),
                }
                let metadata = std::fs::symlink_metadata(&path)
                    .map_err(|error| failure(Code::PersistenceFailed, error.to_string()))?;
                if !metadata.file_type().is_dir() {
                    return Err(failure(
                        Code::OperationConflict,
                        "WorkHub workspace must be a real directory",
                    ));
                }
                let cwd = maka_fs_tools::workspace::project::host_path(&path)
                    .map_err(|error| failure(Code::OperationConflict, error.to_string()))?
                    .to_owned();
                Ok(WorkspaceProjection {
                    target: WorkspaceTarget::HostPath { path: cwd.clone() },
                    host_cwd: cwd,
                })
            })
            .await
            .map_err(|error| error.to_string())
        })
        .map_err(|error| failure(Code::OperationUnavailable, error.to_string()))?;
    pending
        .await
        .map_err(|error| failure(Code::InternalFailure, error.to_string()))?
        .map_err(|error| failure(Code::InternalFailure, error))?
}
