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
    handshake::{HostHandshake, Lifecycle, decode_host_handshake},
    host::{RetirementInput, RetirementResult, decode_retirement_result},
};
use maka_runtime_host::server::{HostError, HostOperations};
use maka_transport::ndjson::{NdjsonReader, NdjsonWriter};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::Read,
    num::NonZeroU32,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{ReadHalf, WriteHalf};
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
type Stream = tokio::net::UnixStream;
#[cfg(windows)]
type Stream = tokio::net::windows::named_pipe::NamedPipeClient;

/// Discovery is only a hint. The live handshake must confirm both root and epoch.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    root_id: String,
    host_epoch: String,
    endpoint: PathBuf,
    pid: NonZeroU32,
}

pub(super) struct HostClient {
    discovery: Discovery,
    reader: NdjsonReader<ReadHalf<Stream>>,
    writer: NdjsonWriter<WriteHalf<Stream>>,
}

impl HostClient {
    pub async fn connect(root: &Path) -> Result<Self, HostError> {
        let root = root.to_owned();
        tokio::time::timeout(Duration::from_secs(5), async move {
            let discovery = tokio::task::spawn_blocking(move || read_discovery(&root)).await??;
            #[cfg(unix)]
            let stream = Stream::connect(&discovery.endpoint).await?;
            #[cfg(windows)]
            let stream = {
                use tokio::net::windows::named_pipe::ClientOptions;
                loop {
                    match ClientOptions::new().open(&discovery.endpoint) {
                        Ok(stream) => break stream,
                        // An existing pipe instance may be between accepts.
                        Err(error) if error.raw_os_error() == Some(231) =>
                            tokio::time::sleep(Duration::from_millis(10)).await,
                        Err(error) => return Err(error.into()),
                    }
                }
            };
            let (mut reader, mut writer) = maka_transport::ndjson::split(stream, CancellationToken::new());
            writer.write(&json!({
                "kind":"hello", "clientInstanceId":format!("maka-operator-{}", uuid::Uuid::new_v4()),
                "surface":"cli", "activitySnapshotVersion":2,
                "protocolMin":0, "protocolMax":0, "compatibilityEpoch":COMPATIBILITY_EPOCH,
                "compositionId":COMPOSITION_ID,
            })).await?;
            let hello = reader.read().await?.ok_or("Host closed before handshake")?;
            match decode_host_handshake(&hello)? {
                HostHandshake::Accepted {
                    root_id, host_epoch, composition_id, selected_protocol,
                    compatibility_epoch, state: Lifecycle::Ready, ..
                } if root_id == discovery.root_id && host_epoch == discovery.host_epoch
                    && composition_id == COMPOSITION_ID && selected_protocol == 0
                    && compatibility_epoch == COMPATIBILITY_EPOCH => {}
                _ => return Err("Host discovery is stale, incompatible or not ready".into()),
            }
            Ok(Self { discovery, reader, writer })
        }).await?
    }

    pub async fn status(&mut self) -> Result<Value, HostError> {
        let result = self.request(Operation::HostStatus, json!({})).await?;
        if result["hostEpoch"] != self.discovery.host_epoch {
            return Err("Host epoch changed during status query".into());
        }
        Ok(result)
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

    async fn request(&mut self, operation: Operation, input: Value) -> Result<Value, HostError> {
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
