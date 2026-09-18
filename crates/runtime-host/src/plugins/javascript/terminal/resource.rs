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

use super::super::{
    invocation::{Authority, Ticket},
    process::Lifetime,
};
use super::{Spawn, message};
use crate::{execution::Executions, shell::ShellHandle};
use maka_runtime::{
    shell_run::{ShellOutput, ShellRun, ShellState, ShellVisibility},
    terminal::TerminalScreen,
};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(super) struct Worker {
    pub host: Arc<Executions>,
    pub authority: Authority,
    pub input: Spawn,
    pub id: String,
    pub ticket: Option<Ticket>,
    pub stop: CancellationToken,
    pub retiring: CancellationToken,
    pub send: oneshot::Sender<Result<ShellHandle, String>>,
}
impl Worker {
    pub async fn run(mut self) -> Result<(), String> {
        let start = self.start().await;
        let mut shell = match start {
            Ok(shell) => shell,
            Err(error) => {
                let _ = self.send.send(Err(error));
                return Ok(());
            }
        };
        if let Some(ticket) = &mut self.ticket {
            ticket.start();
        }
        if self.send.send(Ok(shell.clone())).is_err() {
            self.stop.cancel();
        }
        let invocation = match self.input.command.lifetime {
            Lifetime::Invocation => self.authority.cancellation,
            Lifetime::Instance => CancellationToken::new(),
        };
        let result = tokio::select! {
            biased;
            _ = self.stop.cancelled() => None,
            _ = self.retiring.cancelled() => None,
            _ = invocation.cancelled() => None,
            result = shell.drained() => Some(result.map_err(message)),
        };
        let result = match result {
            Some(result) => result.map(|_| ()),
            None => {
                shell.stop();
                tokio::time::timeout(Duration::from_secs(4), shell.drained())
                    .await
                    .map_err(|_| "terminal cleanup unconfirmed".to_owned())
                    .and_then(|result| result.map(|_| ()).map_err(message))
            }
        };
        if result.is_err() {
            self.host.begin_drain();
        }
        if let Some(ticket) = self.ticket {
            ticket.complete(result.clone());
        }
        result
    }
    async fn start(&self) -> Result<ShellHandle, String> {
        if self.stop.is_cancelled()
            || self.retiring.is_cancelled()
            || self.authority.cancellation.is_cancelled()
        {
            return Err("terminal launch cancelled".into());
        }
        let invocation = &self.authority.identity.invocation;
        let (cwd, _gate) = self
            .host
            .admit_plugin_process(invocation)
            .await
            .map_err(message)?;
        if self.stop.is_cancelled()
            || self.retiring.is_cancelled()
            || self.authority.cancellation.is_cancelled()
        {
            return Err("terminal launch cancelled".into());
        }
        let command = self.input.command.prepare(&cwd)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(message)?
            .as_millis() as u64;
        let record = ShellRun {
            id: self.id.clone(),
            session_id: invocation.session_id.clone(),
            source_run_id: Some(invocation.run_id.clone()),
            source_turn_id: invocation.turn_id.clone(),
            source_tool_call_id: self
                .authority
                .identity
                .operation_id
                .as_ref()
                .map(|id| maka_runtime::tool_call::tool_use_id(&invocation.invocation_id, id))
                .unwrap_or_else(|| format!("plugin:{}", self.id)),
            visibility: ShellVisibility::Model,
            cwd,
            command: serde_json::to_string(&(
                &self.input.command.executable,
                &self.input.command.args,
            ))
            .map_err(message)?,
            started_at: now,
            updated_at: now,
            timeout_ms: None,
            revision: 1,
            state: ShellState::Starting,
            output: ShellOutput::Pty {
                screen: TerminalScreen::new(self.input.size),
            },
        };
        self.host
            .shells
            .start_pty(record, command, self.input.size)
            .map_err(message)
    }
}
