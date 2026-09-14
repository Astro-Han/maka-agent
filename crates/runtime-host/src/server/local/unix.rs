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

use super::HostError;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::net::{UnixListener, UnixStream};

/// Refuses to overwrite an existing socket. Its owner cleans up only its own
/// inode after all connections have drained.
pub struct LocalListener {
    listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
}

impl LocalListener {
    pub fn bind(path: &Path) -> Result<Self, HostError> {
        // Socket mode is not an atomic bind option: the enclosing directory
        // must already exclude other accounts before the socket exists.
        let parent = path
            .parent()
            .ok_or("socket parent directory required")?
            .canonicalize()?;
        let directory = parent.metadata()?;
        if directory.uid() != unsafe { libc::geteuid() } || directory.mode() & 0o077 != 0 {
            return Err("socket parent must be a private current-account directory".into());
        }
        let path = parent.join(path.file_name().ok_or("socket filename required")?);
        use std::os::unix::ffi::OsStrExt;
        if path.as_os_str().as_bytes().len() > 100 {
            return Err("Unix socket path exceeds portable 100-byte limit".into());
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let meta = path.symlink_metadata()?;
        Ok(Self {
            listener,
            path,
            identity: (meta.dev(), meta.ino()),
        })
    }

    pub(in crate::server) async fn accept(&mut self) -> std::io::Result<UnixStream> {
        self.listener.accept().await.map(|(socket, _)| socket)
    }
}

impl Drop for LocalListener {
    fn drop(&mut self) {
        if self
            .path
            .symlink_metadata()
            .is_ok_and(|m| (m.dev(), m.ino()) == self.identity)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
