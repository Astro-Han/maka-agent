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

//! Read-only local discovery shared by interactive and operator clients.
use maka_event_log::root::{self, RootNamespaces};
use serde::Deserialize;
#[cfg(windows)]
use std::time::Duration;
use std::{
    io::Read,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

#[cfg(unix)]
pub type Stream = tokio::net::UnixStream;
#[cfg(windows)]
pub type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

/// Discovery is only a hint. The live handshake must confirm both root and epoch.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub root_id: String,
    pub generation: Option<String>,
    pub host_epoch: String,
    pub endpoint: PathBuf,
    pub pid: NonZeroU32,
    #[serde(default)]
    pub websocket_endpoints: Vec<String>,
}

pub async fn open_stream(endpoint: &Path) -> Result<Stream, crate::Error> {
    #[cfg(unix)]
    return Ok(Stream::connect(endpoint).await?);
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        loop {
            match ClientOptions::new().open(endpoint) {
                Ok(stream) => return Ok(stream),
                // An existing pipe instance may be between accepts.
                Err(error) if error.raw_os_error() == Some(231) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

pub fn read_discovery(path: &Path) -> Result<Discovery, crate::Error> {
    let root = root::resolve(path)?;
    let path = RootNamespaces::for_current_account()?
        .control
        .join(root.root_id())
        .join("registration.json");
    let before = path.symlink_metadata()?;
    if !before.is_file() || before.len() > 16 * 1024 {
        return Err("Invalid Host discovery record".into());
    }
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?
    };
    #[cfg(windows)]
    let file = maka_event_log::root::windows::open_nofollow(&path, false)?;
    if !file.metadata()?.is_file() {
        return Err("Host discovery is not a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
    let after = path.symlink_metadata()?;
    if !after.is_file()
        || bytes.len() > 16 * 1024
        || before.len() != bytes.len() as u64
        || before.len() != after.len()
        || before.modified()? != after.modified()?
    {
        return Err("Host discovery changed while reading".into());
    }
    let discovery: Discovery = serde_json::from_slice(&bytes)?;
    if discovery.root_id != root.root_id()
        || discovery.host_epoch.is_empty()
        || discovery.host_epoch.len() > 128
        || !discovery.endpoint.is_absolute()
    {
        return Err("Host discovery does not match the native root".into());
    }
    #[cfg(windows)]
    if !discovery
        .endpoint
        .to_str()
        .and_then(|path| path.strip_prefix(r"\\.\pipe\"))
        .is_some_and(|name| !name.is_empty() && !name.contains(['\\', '/', '\0']))
    {
        return Err("Host discovery is not a local named pipe".into());
    }
    Ok(discovery)
}
