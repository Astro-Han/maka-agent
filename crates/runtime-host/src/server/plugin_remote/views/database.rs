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

mod query;
use super::{Access, SessionConfiguration, SessionViews};
use maka_plugins::{
    composition::Scope,
    filesystem::{
        OpenFile, ReadError, Symlinks,
        database::{self as api, Error},
    },
};
use std::{
    path::Path,
    sync::{Arc, LazyLock},
};
use tokio::sync::Semaphore;

static READS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

pub(super) async fn read(views: &SessionViews, input: api::Read) -> Result<Vec<api::Table>, Error> {
    input.validate()?;
    if views.access != Access::HostPaths {
        return Err(Error::Denied);
    }
    let lease = views.owner.resource_call().map_err(|_| Error::Denied)?;
    let stopping = views.owner.stopping().map_err(|_| Error::Denied)?;
    let host = views.host().await.map_err(remote_error)?;
    let identity = views.owner.identity().map_err(|_| Error::Denied)?;
    let path = tokio::fs::canonicalize(&input.path)
        .await
        .map_err(io_error)?;
    // Database reads never expose Host authority, even from an unrestricted Session.
    if path.starts_with(host.root.canonical_path()) {
        return Err(Error::Denied);
    }
    let scoped = matches!(identity.scope, Scope::Session(_));
    let workspace = if scoped {
        let id = views.session_id.as_ref().ok_or(Error::Denied)?;
        host.log
            .get_session::<SessionConfiguration>(id)
            .await
            .map_err(|error| Error::Unavailable(error.to_string()))?
            .ok_or(Error::Denied)?
            .configuration
            .workspace
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| Error::Invalid("database has no parent directory".into()))?;
        let parent = maka_fs_tools::workspace::project::host_path(parent)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        crate::server::resolve_workspace_path(parent.to_owned())
            .await
            .map_err(|error| Error::Invalid(error.message))?
    };
    let host_path = maka_fs_tools::workspace::project::host_path(&path)
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let relative = Path::new(host_path)
        .strip_prefix(Path::new(&workspace.host_cwd))
        .map_err(|_| Error::Denied)?
        .components()
        .map(|part| match part {
            std::path::Component::Normal(name) => name.to_str().ok_or(Error::Denied),
            _ => Err(Error::Denied),
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("/");
    let files = views
        .files(&workspace, scoped)
        .await
        .map_err(remote_error)?;
    let names = relative.clone();
    files
        .with_reader(move |reader| {
            reader.file_info(OpenFile {
                path: names.clone(),
                symlinks: Symlinks::Reject,
            })?;
            for suffix in ["-wal", "-shm", "-journal"] {
                match reader.file_info(OpenFile {
                    path: format!("{names}{suffix}"),
                    symlinks: Symlinks::Reject,
                }) {
                    Ok(_) => {}
                    Err(ReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok::<_, ReadError>(())
        })
        .await
        .map_err(read_error)?
        .map_err(read_error)?;
    let permit = READS.clone().try_acquire_owned().map_err(|_| Error::Busy)?;
    let mut ticket = views.resources.reserve().map_err(|_| Error::Cancelled)?;
    let cancelled = views.cancellation.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let _permit = permit;
        ticket.start();
        let result = query::read(&path, input.queries, &cancelled, &stopping);
        // A rejected read has no unresolved external effect. Close happens
        // inside query::read, before releasing admission or acknowledging cleanup.
        ticket.complete(Ok(()));
        result
    })
    .await
    .map_err(|error| Error::Unavailable(error.to_string()))?;
    // Retain the original ReadGrant: a changed boundary or revoked credential
    // cannot be replaced by a newly authorized scope after work completed.
    files
        .file_info(OpenFile {
            path: relative,
            symlinks: Symlinks::Reject,
        })
        .await
        .map_err(read_error)?;
    result
}
fn io_error(error: std::io::Error) -> Error {
    match error.kind() {
        std::io::ErrorKind::NotFound => Error::NotFound,
        std::io::ErrorKind::PermissionDenied => Error::Denied,
        _ => Error::Unavailable(error.to_string()),
    }
}
fn read_error(error: ReadError) -> Error {
    match error {
        ReadError::Retired => Error::Cancelled,
        ReadError::Io(error) => io_error(error),
        ReadError::Invalid(_) => Error::Denied,
        ReadError::ScanLimit { .. } => Error::Limit(api::Limit::Work),
    }
}
fn remote_error(error: super::Error) -> Error {
    match error {
        super::Error::Retired => Error::Denied,
        super::Error::Cancelled => Error::Cancelled,
        super::Error::Invalid(message) => Error::Invalid(message),
        error => Error::Unavailable(error.to_string()),
    }
}
