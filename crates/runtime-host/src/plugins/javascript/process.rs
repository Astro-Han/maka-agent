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

mod worker;

use super::invocation::Authority;
use crate::execution::Executions;
use maka_plugins::fiber::Context;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Lifetime {
    #[default]
    Invocation,
    Instance,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Spawn {
    pub authority: String,
    pub command: Command,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Command {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub lifetime: Lifetime,
}
impl Command {
    pub fn prepare(&self, cwd: &str) -> Result<maka_process::Command, String> {
        if !std::path::Path::new(&self.executable).is_absolute()
            || self.args.len() > 256
            || self.env.len() > 128
            || serde_json::to_vec(&(&self.executable, &self.args, &self.env))
                .map_err(message)?
                .len()
                > 64 * 1024
            || self.executable.contains('\0')
            || self.args.iter().any(|arg| arg.contains('\0'))
            || self.env.iter().any(|(key, value)| {
                key.is_empty() || key.contains(['=', '\0']) || value.contains('\0')
            })
        {
            return Err("invalid process command or launch limit exceeded".into());
        }
        let mut command = maka_process::Command::new(&self.executable, cwd);
        command.args(&self.args);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        Ok(command)
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Exit {
    pub code: Option<i32>,
    pub success: bool,
    pub stopped: bool,
    pub error: Option<String>,
}
#[derive(Clone)]
enum State {
    Starting,
    Running,
    Ended(Result<Exit, String>),
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Stream {
    Stdout,
    Stderr,
}
#[derive(Serialize)]
pub(super) struct Chunk {
    pub stream: Stream,
    pub bytes: Vec<u8>,
}

struct Handle {
    session: String,
    input: mpsc::Sender<Option<Vec<u8>>>,
    output: tokio::sync::Mutex<mpsc::Receiver<Chunk>>,
    state: watch::Receiver<State>,
    stop: CancellationToken,
    expires: Option<CancellationToken>,
}
pub(super) struct Processes {
    host: Weak<Executions>,
    owner: Context,
    handles: Mutex<BTreeMap<String, Arc<Handle>>>,
}
impl Processes {
    pub fn new(host: Weak<Executions>, owner: Context) -> Self {
        Self {
            host,
            owner,
            handles: Mutex::default(),
        }
    }
    pub async fn spawn(&self, authority: Authority, input: Command) -> Result<String, String> {
        let _lease = self.owner.admit().map_err(message)?;
        let host = self.host.upgrade().ok_or("Host closed")?;
        let (cwd, admission) = host
            .admit_plugin_process(&authority.identity.invocation)
            .await
            .map_err(message)?;
        let command = input.prepare(&cwd)?;
        let ticket = match input.lifetime {
            Lifetime::Invocation => Some(authority.resources.reserve().map_err(message)?),
            Lifetime::Instance => None,
        };
        let (send, output) = mpsc::channel(8);
        let (stdin, receive) = mpsc::channel(8);
        let (state, snapshot) = watch::channel(State::Starting);
        let stop = CancellationToken::new();
        let unclaimed = stop.clone().drop_guard();
        let handle = Arc::new(Handle {
            session: authority.identity.invocation.session_id,
            input: stdin,
            output: tokio::sync::Mutex::new(output),
            state: snapshot,
            stop: stop.clone(),
            expires: matches!(input.lifetime, Lifetime::Invocation)
                .then(|| authority.cancellation.clone()),
        });
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut handles = self.handles.lock().unwrap();
            handles.retain(|_, handle| {
                !handle.stop.is_cancelled()
                    && !handle
                        .expires
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled)
            });
            if handles.len() >= 32 {
                return Err("plugin process handle capacity exceeded; close unused handles".into());
            }
            if authority.cancellation.is_cancelled() {
                return Err("invocation closed".into());
            }
            handles.insert(id.clone(), handle.clone());
        }
        let lifetime = input.lifetime;
        let activity = host.own_plugin_process(&handle.session);
        let execution = self
            .owner
            .spawn_resource("protocol process", move |retiring| async move {
                let _activity = activity;
                let mut ticket = ticket;
                if let Some(ticket) = &mut ticket {
                    ticket.start();
                }
                let launch = authority.cancellation.clone();
                let cancellation = match lifetime {
                    Lifetime::Invocation => authority.cancellation,
                    Lifetime::Instance => CancellationToken::new(),
                };
                let result = worker::run(
                    command,
                    receive,
                    send,
                    state,
                    worker::Stops {
                        launch,
                        explicit: stop,
                        retiring,
                        invocation: cancellation,
                    },
                    admission,
                )
                .await;
                if result.is_err() {
                    host.begin_drain();
                }
                if let Some(ticket) = ticket {
                    ticket.complete(result.clone());
                }
                result
            });
        if let Err(error) = execution {
            self.handles.lock().unwrap().remove(&id);
            return Err(error.to_string());
        }
        let mut state = handle.state.clone();
        loop {
            let current = state.borrow_and_update().clone();
            match current {
                State::Starting => state.changed().await.map_err(message)?,
                State::Running | State::Ended(Ok(_)) => {
                    unclaimed.disarm();
                    return Ok(id);
                }
                State::Ended(Err(error)) => {
                    self.handles.lock().unwrap().remove(&id);
                    return Err(error);
                }
            }
        }
    }

    async fn get(&self, authority: &Authority, id: &str) -> Result<Arc<Handle>, String> {
        if authority.cancellation.is_cancelled() {
            return Err("invocation closed".into());
        }
        let handle = self
            .handles
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("process handle closed")?;
        if handle
            .expires
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err("process invocation closed".into());
        }
        if handle.session != authority.identity.invocation.session_id {
            return Err("process belongs to another Session".into());
        }
        self.host
            .upgrade()
            .ok_or("Host closed")?
            .plugin_process_workspace(&authority.identity.invocation)
            .await
            .map_err(message)?;
        Ok(handle)
    }
    pub async fn write(
        &self,
        authority: &Authority,
        id: &str,
        bytes: Vec<u8>,
    ) -> Result<(), String> {
        if bytes.len() > 64 * 1024 {
            return Err("process input exceeds 64 KiB".into());
        }
        self.get(authority, id)
            .await?
            .input
            .try_send(Some(bytes))
            .map_err(message)
    }
    pub async fn end_input(&self, authority: &Authority, id: &str) -> Result<(), String> {
        self.get(authority, id)
            .await?
            .input
            .try_send(None)
            .map_err(message)
    }
    pub async fn next(&self, authority: &Authority, id: &str) -> Result<Option<Chunk>, String> {
        let handle = self.get(authority, id).await?;
        let mut output = handle
            .output
            .try_lock()
            .map_err(|_| "process already has an output consumer")?;
        tokio::select! {
            biased;
            _ = authority.cancellation.cancelled() => Err("invocation closed".into()),
            chunk = output.recv() => Ok(chunk),
        }
    }
    pub async fn wait(&self, authority: &Authority, id: &str) -> Result<Exit, String> {
        let mut state = self.get(authority, id).await?.state.clone();
        loop {
            if let State::Ended(result) = state.borrow_and_update().clone() {
                return result;
            }
            tokio::select! {
                biased;
                _ = authority.cancellation.cancelled() => return Err("invocation closed".into()),
                changed = state.changed() => changed.map_err(message)?,
            }
        }
    }
    pub async fn close(&self, id: &str) -> Result<(), String> {
        let Some(handle) = self.handles.lock().unwrap().remove(id) else {
            return Ok(());
        };
        handle.stop.cancel();
        let mut state = handle.state.clone();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let State::Ended(result) = state.borrow_and_update().clone() {
                    return result.map(|_| ());
                }
                state.changed().await.map_err(message)?;
            }
        })
        .await
        .map_err(|_| "process cleanup unconfirmed")?
    }
}
fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
