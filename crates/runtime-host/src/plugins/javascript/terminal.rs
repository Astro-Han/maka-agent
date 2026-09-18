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

use super::{
    invocation::Authority,
    process::{Command, Lifetime},
};
use crate::{
    execution::Executions,
    shell::{PtyReplay, PtyStream, PtyStreamEvent, ShellHandle},
};
use maka_plugins::fiber::Context;
use maka_runtime::{shell_run::ShellOutcome, terminal::TerminalSize};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

mod resource;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Spawn {
    pub authority: String,
    pub command: Command,
    pub size: TerminalSize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Control {
    pub authority: String,
    pub handle: String,
    #[serde(default)]
    pub text: String,
    pub size: Option<TerminalSize>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Written {
    pub accepted_bytes: usize,
    pub resized: bool,
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Output {
    Data {
        sequence: u64,
        text: String,
    },
    Reset {
        sequence: u64,
        text: String,
        size: TerminalSize,
    },
    Closed,
}
struct Cursor {
    initial: Option<PtyReplay>,
    stream: PtyStream,
}
struct Handle {
    session: String,
    expires: Option<CancellationToken>,
    stop: CancellationToken,
    shell: ShellHandle,
    cursor: tokio::sync::Mutex<Cursor>,
    _capacity: tokio::sync::OwnedSemaphorePermit,
}
pub(super) struct Terminals {
    host: Weak<Executions>,
    owner: Context,
    handles: Mutex<BTreeMap<String, Arc<Handle>>>,
    capacity: Arc<tokio::sync::Semaphore>,
}
impl Terminals {
    pub fn new(host: Weak<Executions>, owner: Context) -> Self {
        Self {
            host,
            owner,
            handles: Mutex::default(),
            capacity: Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }
    pub async fn spawn(&self, authority: Authority, input: Spawn) -> Result<String, String> {
        let lease = self.owner.admit().map_err(message)?;
        self.handles.lock().unwrap().retain(|_, handle| {
            !handle
                .expires
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        });
        let capacity = self
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| "plugin terminal capacity exceeded; close unused handles")?;
        let host = self.host.upgrade().ok_or("Host closed")?;
        let invocation = authority.identity.invocation.clone();
        let expires = matches!(input.command.lifetime, Lifetime::Invocation)
            .then(|| authority.cancellation.clone());
        let ticket = match input.command.lifetime {
            Lifetime::Invocation => Some(authority.resources.reserve().map_err(message)?),
            Lifetime::Instance => None,
        };
        let stop = CancellationToken::new();
        let terminal_id = uuid::Uuid::new_v4().to_string();
        let (send, receive) = tokio::sync::oneshot::channel();
        let worker_stop = stop.clone();
        let id = terminal_id.clone();
        self.owner
            .spawn_resource("terminal", move |retiring| async move {
                let _lease = lease;
                resource::Worker {
                    host,
                    authority,
                    input,
                    id,
                    ticket,
                    stop: worker_stop,
                    retiring,
                    send,
                }
                .run()
                .await
            })
            .map_err(message)?;
        let shell = receive.await.map_err(|_| "terminal worker disappeared")??;
        let (initial, stream) = shell.attach().ok_or("terminal has no output")?;
        let handle = Arc::new(Handle {
            session: invocation.session_id,
            expires,
            stop,
            shell,
            cursor: tokio::sync::Mutex::new(Cursor {
                initial: Some(initial),
                stream,
            }),
            _capacity: capacity,
        });
        self.handles
            .lock()
            .unwrap()
            .insert(terminal_id.clone(), handle.clone());
        if let Err(error) = handle.shell.clone().ready().await {
            self.handles.lock().unwrap().remove(&terminal_id);
            handle.stop.cancel();
            return Err(message(error));
        }
        Ok(terminal_id)
    }
    async fn get(&self, authority: &Authority, id: &str) -> Result<Arc<Handle>, String> {
        let _lease = self.owner.admit().map_err(message)?;
        if authority.cancellation.is_cancelled() {
            return Err("invocation closed".into());
        }
        let handle = self
            .handles
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("terminal closed")?;
        if handle.session != authority.identity.invocation.session_id
            || handle
                .expires
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
        {
            return Err("terminal belongs to another or closed invocation".into());
        }
        self.host
            .upgrade()
            .ok_or("Host closed")?
            .plugin_process_workspace(&authority.identity.invocation)
            .await
            .map_err(message)?;
        Ok(handle)
    }
    pub async fn control(&self, authority: &Authority, input: Control) -> Result<Written, String> {
        if input.text.len() > 64 * 1024 || (input.text.is_empty() && input.size.is_none()) {
            return Err("terminal input is empty or exceeds 64 KiB".into());
        }
        let handle = self.get(authority, &input.handle).await?;
        let receipt = handle
            .shell
            .control(
                crate::shell::ControlInput::Raw(input.text),
                input.size,
                authority.cancellation.clone(),
            )
            .await
            .map_err(message)?;
        Ok(Written {
            accepted_bytes: receipt.accepted_bytes,
            resized: receipt.resized,
        })
    }
    pub async fn next(&self, authority: &Authority, id: &str) -> Result<Output, String> {
        let handle = self.get(authority, id).await?;
        let mut cursor = handle
            .cursor
            .try_lock()
            .map_err(|_| "terminal already has an output consumer")?;
        let event = if let Some(initial) = cursor.initial.take() {
            PtyStreamEvent::Reset(initial)
        } else {
            tokio::select! {
                biased;
                _ = authority.cancellation.cancelled() => return Err("invocation closed".into()),
                event = cursor.stream.next() => event,
            }
        };
        Ok(match event {
            PtyStreamEvent::Data(data) => Output::Data {
                sequence: data.sequence,
                text: data.data.clone(),
            },
            PtyStreamEvent::Reset(data) => Output::Reset {
                sequence: data.sequence,
                text: data.buffer,
                size: data.size,
            },
            PtyStreamEvent::Closed => Output::Closed,
        })
    }
    pub async fn wait(&self, authority: &Authority, id: &str) -> Result<ShellOutcome, String> {
        let handle = self.get(authority, id).await?;
        let mut shell = handle.shell.clone();
        let record = tokio::select! {
            biased;
            _ = authority.cancellation.cancelled() => return Err("invocation closed".into()),
            record = shell.drained() => record.map_err(message)?,
        };
        match &record.state {
            maka_runtime::shell_run::ShellState::Terminal { outcome, .. } => Ok(outcome.clone()),
            _ => Err("terminal worker exited without settlement".into()),
        }
    }
    pub async fn close(&self, id: &str) -> Result<(), String> {
        let Some(handle) = self.handles.lock().unwrap().remove(id) else {
            return Ok(());
        };
        handle.stop.cancel();
        tokio::time::timeout(Duration::from_secs(8), handle.shell.clone().drained())
            .await
            .map_err(|_| "terminal cleanup unconfirmed")?
            .map_err(message)?;
        Ok(())
    }
}
fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
