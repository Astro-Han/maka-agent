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

use super::super::{Result, failure};
use crate::server::Host;
use maka_protocol::OperationErrorCode as Code;
use std::{
    io,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{fs::File, io::AsyncWrite};

pub(super) struct Output {
    file: File,
    temporary: tempfile::NamedTempFile,
    destination: PathBuf,
}

impl Output {
    pub async fn open(host: &Host, path: &str) -> Result<Self> {
        let destination = destination(host, path).await?;
        let target = destination.clone();
        let temporary = tokio::task::spawn_blocking(move || {
            tempfile::NamedTempFile::new_in(target.parent().expect("validated parent"))
        })
        .await
        .map_err(join)?
        .map_err(write)?;
        let file = File::from_std(temporary.as_file().try_clone().map_err(write)?);
        Ok(Self {
            file,
            temporary,
            destination,
        })
    }

    pub async fn publish(self) -> Result<u64> {
        let Self {
            file,
            temporary,
            destination,
        } = self;
        file.sync_all().await.map_err(write)?;
        let bytes = file.metadata().await.map_err(write)?.len();
        drop(file.into_std().await);
        tokio::task::spawn_blocking(move || {
            temporary
                .persist_noclobber(&destination)
                .map_err(|e| write(e.error))?;
            #[cfg(unix)]
            std::fs::File::open(destination.parent().expect("validated parent"))
                .and_then(|directory| directory.sync_all())
                .map_err(write)?;
            Ok::<_, maka_protocol::OperationError>(bytes)
        })
        .await
        .map_err(join)?
    }
}

impl AsyncWrite for Output {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.file).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_shutdown(cx)
    }
}

pub(super) async fn input(host: &Host, path: &str) -> Result<File> {
    let path = tokio::fs::canonicalize(path).await.map_err(read)?;
    permitted(host, &path)?;
    if !tokio::fs::metadata(&path).await.map_err(read)?.is_file() {
        return Err(failure(
            Code::SourceUnreadable,
            "Bundle must be a regular file",
        ));
    }
    let file = File::open(path).await.map_err(read)?;
    let metadata = file.metadata().await.map_err(read)?;
    if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 * 1024 {
        return Err(failure(
            Code::SourceUnreadable,
            "Bundle must be a regular file of at most 2 GiB",
        ));
    }
    Ok(file)
}

async fn destination(host: &Host, path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err(failure(
            Code::InvalidRequest,
            "Bundle path must be absolute",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| failure(Code::InvalidRequest, "Bundle path has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| failure(Code::InvalidRequest, "Bundle path has no filename"))?;
    let destination = tokio::fs::canonicalize(parent)
        .await
        .map_err(write)?
        .join(name);
    permitted(host, &destination)?;
    match tokio::fs::symlink_metadata(&destination).await {
        Ok(_) => {
            return Err(failure(
                Code::OperationConflict,
                "Bundle destination already exists",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(write(error)),
    }
    Ok(destination)
}

fn permitted(host: &Host, path: &Path) -> Result<()> {
    if path.starts_with(host.root.canonical_path()) || path.starts_with(host.control_directory()) {
        return Err(failure(
            Code::InvalidRequest,
            "Session bundles cannot access Host state directories",
        ));
    }
    Ok(())
}

fn read(error: io::Error) -> maka_protocol::OperationError {
    failure(Code::SourceUnreadable, &error.to_string())
}
fn write(error: io::Error) -> maka_protocol::OperationError {
    failure(
        if error.kind() == io::ErrorKind::AlreadyExists {
            Code::OperationConflict
        } else {
            Code::PersistenceFailed
        },
        &error.to_string(),
    )
}
fn join(error: tokio::task::JoinError) -> maka_protocol::OperationError {
    failure(Code::InternalFailure, &error.to_string())
}
