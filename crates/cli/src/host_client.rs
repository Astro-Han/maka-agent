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

use maka_event_log::root::{self, RootNamespaces};
use maka_protocol::{
    COMPATIBILITY_EPOCH, COMPOSITION_ID, Operation, Outcome, Request,
    handshake::{ClientHello, HostHandshake, Lifecycle, decode_host_handshake},
    host::{RetirementInput, RetirementResult, decode_retirement_result},
};
use maka_runtime_host::server::{HostError, HostOperations};
use maka_transport::ndjson::{NdjsonReader, NdjsonWriter};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::Read,
    num::{NonZeroU16, NonZeroU32},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{ReadHalf, WriteHalf};
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
pub(super) type Stream = tokio::net::UnixStream;
#[cfg(windows)]
pub(super) type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

/// Discovery is only a hint. The live handshake must confirm both root and epoch.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    root_id: String,
    host_epoch: String,
    endpoint: PathBuf,
    pid: NonZeroU32,
    #[serde(default)]
    websocket_endpoints: Vec<String>,
}

#[derive(serde::Serialize)]
pub(super) struct LiveHost {
    #[serde(rename = "hostEpoch")]
    pub epoch: String,
    pub pid: NonZeroU32,
    pub port: NonZeroU16,
}

pub(super) struct HostClient {
    discovery: Discovery,
    reader: NdjsonReader<ReadHalf<Stream>>,
    writer: NdjsonWriter<WriteHalf<Stream>>,
}

impl HostClient {
    pub fn root_id(&self) -> &str {
        &self.discovery.root_id
    }

    pub async fn connect(root: &Path, generation: Option<&str>) -> Result<Self, HostError> {
        let root = root.to_owned();
        tokio::time::timeout(Duration::from_secs(5), async move {
            let discovery = tokio::task::spawn_blocking(move || read_discovery(&root)).await??;
            let stream = open_stream(&discovery.endpoint).await?;
            let (mut reader, mut writer) =
                maka_transport::ndjson::split(stream, CancellationToken::new());
            writer
                .write(&ClientHello {
                    client_instance_id: format!("maka-operator-{}", uuid::Uuid::new_v4()),
                    activity_snapshot_version: Some(2),
                    protocol_min: 0,
                    protocol_max: 0,
                    compatibility_epoch: COMPATIBILITY_EPOCH,
                    composition_id: COMPOSITION_ID.into(),
                    generation: generation.map(str::to_owned),
                    takeover: None,
                })
                .await?;
            let hello = reader.read().await?.ok_or("Host closed before handshake")?;
            match decode_host_handshake(&hello)? {
                HostHandshake::Accepted {
                    root_id,
                    host_epoch,
                    composition_id,
                    selected_protocol,
                    compatibility_epoch,
                    state: Lifecycle::Ready,
                    ..
                } if root_id == discovery.root_id
                    && host_epoch == discovery.host_epoch
                    && composition_id == COMPOSITION_ID
                    && selected_protocol == 0
                    && compatibility_epoch == COMPATIBILITY_EPOCH => {}
                _ => return Err("Host discovery is stale, incompatible or not ready".into()),
            }
            Ok(Self {
                discovery,
                reader,
                writer,
            })
        })
        .await?
    }

    /// A fresh connection to this verified epoch, not a second discovery lookup.
    /// The bridge must still check its own handshake before forwarding requests.
    pub async fn open_bridge(&self) -> Result<Stream, HostError> {
        tokio::time::timeout(
            Duration::from_secs(5),
            open_stream(&self.discovery.endpoint),
        )
        .await?
    }

    pub async fn status(&mut self) -> Result<Value, HostError> {
        let result = self.request(Operation::HostStatus, json!({})).await?;
        if result["hostEpoch"] != self.discovery.host_epoch {
            return Err("Host epoch changed during status query".into());
        }
        Ok(result)
    }

    pub async fn live_host(&mut self, configured_port: u16) -> Result<LiveHost, HostError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Identity {
            host_epoch: String,
            pid: NonZeroU32,
            protocol_version: u64,
            compatibility_epoch: u64,
        }
        let result = self
            .request(Operation::HostDiagnosticsQuery, json!({}))
            .await?;
        let identity: Identity = serde_json::from_value(result)?;
        if identity.host_epoch != self.discovery.host_epoch
            || identity.pid != self.discovery.pid
            || identity.protocol_version != 0
            || identity.compatibility_epoch != COMPATIBILITY_EPOCH
        {
            return Err("live Host diagnostics disagree with discovery".into());
        }
        let [endpoint] = self.discovery.websocket_endpoints.as_slice() else {
            return Err("managed Host must publish exactly one WebSocket endpoint".into());
        };
        let port: NonZeroU16 = endpoint
            .strip_prefix("ws://127.0.0.1:")
            .and_then(|value| value.strip_suffix("/runtime-host"))
            .ok_or("managed Host published an invalid WebSocket endpoint")?
            .parse()?;
        if configured_port != 0 && configured_port != port.get() {
            return Err("managed Host listener differs from deployment configuration".into());
        }
        Ok(LiveHost {
            epoch: identity.host_epoch,
            pid: identity.pid,
            port,
        })
    }

    pub async fn retire(
        &mut self,
        expected_epoch: Option<&str>,
        interrupt: bool,
    ) -> Result<RetirementResult, HostError> {
        if expected_epoch.is_some_and(|epoch| epoch != self.discovery.host_epoch) {
            return Err("Host epoch does not match the retirement target".into());
        }
        let input = RetirementInput {
            expected_host_epoch: self.discovery.host_epoch.clone(),
            allow_interrupt_active_tasks: interrupt,
            allow_cooperative_handoff: Some(true),
        };
        let result = decode_retirement_result(
            &self
                .request(Operation::HostUpgradePrepare, serde_json::to_value(input)?)
                .await?,
        )?;
        if let RetirementResult::Prepared { pid } = result
            && pid != self.discovery.pid
        {
            return Err("Retirement receipt belongs to another process".into());
        }
        Ok(result)
    }

    pub(super) async fn request(
        &mut self,
        operation: Operation,
        input: Value,
    ) -> Result<Value, HostError> {
        // A timeout is not evidence of rollback. The caller must rediscover the
        // exact owner before deciding whether any subsequent mutation is safe.
        tokio::time::timeout(Duration::from_secs(15), async {
            let request_id = uuid::Uuid::new_v4().to_string();
            self.writer
                .write(&Request {
                    request_id: request_id.clone(),
                    operation,
                    input,
                })
                .await?;
            loop {
                let frame = self
                    .reader
                    .read()
                    .await?
                    .ok_or("Host closed before its response")?;
                if frame.get("requestId").is_none() {
                    continue;
                }
                let response = maka_protocol::decode_response(&frame, &HostOperations)?;
                if response.request_id != request_id || response.operation != operation {
                    return Err("Host returned an unrelated response".into());
                }
                return match response.outcome {
                    Outcome::Success { result } => Ok(result),
                    Outcome::Failure { error } => Err(error.into()),
                };
            }
        })
        .await?
    }
}

async fn open_stream(endpoint: &Path) -> Result<Stream, HostError> {
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

fn read_discovery(path: &Path) -> Result<Discovery, HostError> {
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
