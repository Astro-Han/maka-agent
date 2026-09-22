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

use super::{Deployment, Mode, RootId, directory, store};
use clap::Args;
use maka_runtime_host::server::HostError;
use serde::{Deserialize, Serialize};

const LIMIT: usize = 48 * 1024;

#[derive(Args)]
pub(crate) struct Logs {
    #[arg(long)]
    root_id: RootId,
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum Output {
    NotCaptured,
    Tail {
        source: Source,
        text: String,
        byte_truncated: bool,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum Source {
    #[cfg(target_os = "linux")]
    Journal { entry_limit: usize },
    #[cfg(any(target_os = "macos", windows))]
    Stderr,
}

impl Logs {
    pub async fn run(self) -> Result<(), HostError> {
        let directory = directory(&self.root_id.0)?;
        let store::Installation::Installed(deployment) = store::read(&directory).await? else {
            return Err("State Root has no installed deployment".into());
        };
        if deployment.root_id != self.root_id.0 {
            return Err("deployment root identity differs".into());
        }
        deployment.validate_record(&directory)?;
        let output = if deployment.mode == Mode::OnDemand {
            Output::NotCaptured
        } else {
            read(&deployment).await?
        };
        println!("{}", serde_json::to_string(&output)?);
        Ok(())
    }
}

#[cfg(any(target_os = "macos", windows))]
async fn read(deployment: &Deployment) -> Result<Output, HostError> {
    let directory = directory(&deployment.root_id)?;
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Seek, SeekFrom};
        let path = directory.join("host.stderr.log");
        #[cfg(target_os = "macos")]
        let opened = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)
        };
        #[cfg(windows)]
        let opened = maka_event_log::root::windows::open_nofollow(&path, false);
        let mut file = match opened {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Output::NotCaptured);
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err("service log is not a regular file".into());
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
                || metadata.nlink() != 1
            {
                return Err("service log is not private and singly linked".into());
            }
        }
        #[cfg(windows)]
        {
            use maka_event_log::root::windows::{file_identity, validate_private};
            validate_private(&file)?;
            if file_identity(&file)?.links != 1 {
                return Err("service log is not singly linked".into());
            }
        }
        // Freeze the end offset; an active writer must not make this a follow.
        let start = metadata.len().saturating_sub(LIMIT as u64);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::with_capacity(LIMIT);
        file.take(metadata.len() - start).read_to_end(&mut bytes)?;
        Ok(Output::Tail {
            source: Source::Stderr,
            text: String::from_utf8_lossy(&bytes).into_owned(),
            byte_truncated: start != 0,
        })
    })
    .await?
}

#[cfg(target_os = "linux")]
async fn read(deployment: &Deployment) -> Result<Output, HostError> {
    use std::{process::Stdio, time::Duration};
    const ENTRIES: usize = 200;
    let mut child = tokio::process::Command::new("journalctl")
        .args([
            "--user",
            "--no-pager",
            "--quiet",
            "--all",
            "--output=short-iso-precise",
        ])
        .arg(format!(
            "--user-unit=org.apache.maka.host.{}.service",
            deployment.root_id
        ))
        .arg(format!("--lines={ENTRIES}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().ok_or("journal stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("journal stderr unavailable")?;
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::try_join!(tail(stdout, LIMIT), tail(stderr, 2048), child.wait())
    })
    .await;
    let (stdout, stderr, status) = match result {
        Ok(Ok(output)) => output,
        result => {
            // Reap the query even on timeout or pipe failure; no pager is spawned.
            let _ = child.kill().await;
            return Err(match result {
                Err(error) => error.into(),
                Ok(Err(error)) => error.into(),
                Ok(Ok(_)) => unreachable!(),
            });
        }
    };
    if !status.success() {
        return Err(format!(
            "journal query failed ({status}): {}",
            String::from_utf8_lossy(&stderr.0)
        )
        .into());
    }
    Ok(Output::Tail {
        source: Source::Journal {
            entry_limit: ENTRIES,
        },
        text: String::from_utf8_lossy(&stdout.0).into_owned(),
        byte_truncated: stdout.1,
    })
}

#[cfg(target_os = "linux")]
async fn tail(
    mut input: impl tokio::io::AsyncRead + Unpin,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    use tokio::io::AsyncReadExt;
    let mut bytes = std::collections::VecDeque::with_capacity(limit);
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let read = input.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let overflow = (bytes.len() + read).saturating_sub(limit);
        if overflow != 0 {
            truncated = true;
            bytes.drain(..overflow.min(bytes.len()));
        }
        bytes.extend(&buffer[read.saturating_sub(limit)..read]);
    }
    Ok((bytes.into(), truncated))
}
