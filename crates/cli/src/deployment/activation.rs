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

use super::{Deployment, Mode, directory, store};
use crate::host_client::{HostClient, LiveHost};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use clap::Args;
use maka_event_log::root::{self, FileLease, RootNamespaces, RootOwner};
use maka_runtime_host::server::HostError;
use serde::Serialize;
use std::{
    io::Write,
    num::{NonZeroU16, NonZeroU32},
    process::Stdio,
    str::FromStr,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
};
use uuid::Uuid;

#[derive(Clone)]
struct RootId(String);

impl FromStr for RootId {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            Ok(Self(value.into()))
        } else {
            Err("root ID must contain 64 lowercase hexadecimal characters")
        }
    }
}

#[derive(Args)]
pub(crate) struct Activate {
    #[arg(long)]
    root_id: RootId,
    #[arg(long)]
    framed: bool,
}

#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Frame {
    Result {
        schema_version: u8,
        deployment_id: Uuid,
        config_revision: u64,
        root_id: String,
        host_epoch: String,
        pid: NonZeroU32,
        protocol_version: u8,
        endpoint: Endpoint,
    },
    Error {
        schema_version: u8,
        error: Failure,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Endpoint {
    host: &'static str,
    port: NonZeroU16,
    websocket_path: &'static str,
}

#[derive(Serialize)]
struct Failure {
    code: &'static str,
    message: String,
}

impl Activate {
    pub async fn run(self) -> Result<(), HostError> {
        let result = self.activate().await;
        if let Err(error) = &result
            && self.framed
        {
            let mut message = error.to_string();
            message.truncate(message.floor_char_boundary(2048));
            emit(
                &Frame::Error {
                    schema_version: 1,
                    error: Failure {
                        code: "activation_failed",
                        message,
                    },
                },
                true,
            )?;
        }
        result
    }

    async fn activate(&self) -> Result<(), HostError> {
        let directory = directory(&self.root_id.0)?;
        // Activation never creates a deployment database.
        if !directory.is_dir() {
            return Err("Host deployment is not installed".into());
        }
        let lease = FileLease::acquire(&directory.join("executor.lock"))?;
        let store::Installation::Installed(deployment) = store::read(&directory).await? else {
            return Err("Host deployment is absent or incomplete".into());
        };
        let root = root::resolve(&deployment.root_path)?;
        if root.root_id() != self.root_id.0 {
            return Err("deployment root identity changed".into());
        }
        deployment.validate(&root, &directory)?;
        lease.validate()?;
        let (client, live) = connect_or_launch(&deployment).await?;
        lease.validate()?;
        emit(
            &Frame::Result {
                schema_version: 1,
                deployment_id: deployment.deployment_id,
                config_revision: deployment.config_revision,
                root_id: deployment.root_id,
                host_epoch: live.epoch,
                pid: live.pid,
                protocol_version: 0,
                endpoint: Endpoint {
                    host: "127.0.0.1",
                    port: live.port,
                    websocket_path: "/runtime-host",
                },
            },
            self.framed,
        )?;
        // Keep the transport resident until the complete result has been flushed.
        drop(client);
        Ok(())
    }
}

async fn connect_or_launch(deployment: &Deployment) -> Result<(HostClient, LiveHost), HostError> {
    let mut child: Option<Child> = None;
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(mut client) =
                HostClient::connect(&deployment.root_path, Some(&deployment.generation())).await
            {
                let live = client.live_host(deployment.websocket.port()).await?;
                return Ok::<_, HostError>((client, live));
            }
            if let Some(child) = &mut child {
                if let Some(status) = child.try_wait()? {
                    return Err(format!("Host candidate exited before Ready: {status}").into());
                }
            } else {
                if deployment.mode != Mode::OnDemand {
                    return Err("the supervised Host must be started by its service".into());
                }
                match RootOwner::open(
                    &deployment.root_path,
                    &RootNamespaces::for_current_account()?,
                ) {
                    Ok(owner) => {
                        if owner.root_id() != deployment.root_id {
                            return Err("State Root changed before launch".into());
                        }
                        drop(owner);
                        let mut command = Command::new(&deployment.executable);
                        command
                            .args(["host", "candidate", "--root"])
                            .arg(&deployment.root_path)
                            .args([
                                "--expected-root-id",
                                &deployment.root_id,
                                "--startup-attempt-id",
                                &Uuid::new_v4().to_string(),
                                "--owner-stdin",
                                "--initial-connection-timeout-ms",
                                "30000",
                            ])
                            .stdin(Stdio::piped())
                            .stdout(Stdio::null())
                            // No inherited SSH capture pipes may outlive the operator.
                            .stderr(Stdio::null());
                        #[cfg(unix)]
                        {
                            // The SSH operator may exit while its released Host continues.
                            // SAFETY: setsid is async-signal-safe and touches no Rust state.
                            unsafe {
                                command.pre_exec(|| {
                                    if libc::setsid() == -1 {
                                        return Err(std::io::Error::last_os_error());
                                    }
                                    Ok(())
                                });
                            }
                        }
                        #[cfg(windows)]
                        command.creation_flags(0x00000008 | 0x00000200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
                        child = Some(command.spawn()?);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(HostError::from)
    .and_then(|result| result);

    match result {
        Ok(connected) => {
            if let Some(child) = &mut child {
                let mut owner = child
                    .stdin
                    .take()
                    .ok_or("candidate owner pipe is missing")?;
                owner
                    .write_all(b"{\"kind\":\"runtime-host-launch-owner-release\"}\n")
                    .await?;
                drop(owner);
            }
            Ok(connected)
        }
        Err(error) => {
            if let Some(mut child) = child {
                // EOF requests the existing orderly drain. Never hard-kill a
                // candidate which may already have recovered committed work.
                drop(child.stdin.take());
                child.wait().await?;
            }
            Err(error)
        }
    }
}

fn emit(frame: &Frame, framed: bool) -> Result<(), HostError> {
    let bytes = serde_json::to_vec(frame)?;
    let line = if framed {
        let encoded = URL_SAFE_NO_PAD.encode(bytes);
        if encoded.len() > 16 * 1024 {
            return Err("activation frame exceeds its limit".into());
        }
        format!("MAKA_RUNTIME_HOST_ACTIVATION_V1 {encoded}\n")
    } else {
        format!("{}\n", String::from_utf8(bytes)?)
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(line.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
