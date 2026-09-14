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

//! Durable shell-resource state, independent of invocation history and live handles.
mod state;
pub use state::{ShellOutcome, ShellState};

use crate::terminal::TerminalScreen;
use serde::{Deserialize, Serialize};

const MAX_SAFE: u64 = 9_007_199_254_740_991;
pub const MAX_SHELL_RECORD_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellVisibility {
    Model,
    User,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipeStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShellOutput {
    Pipes {
        stdout: String,
        stderr: String,
        latest_stream: Option<PipeStream>,
        stdout_truncated: bool,
        stderr_truncated: bool,
    },
    Pty {
        screen: TerminalScreen,
    },
}

impl ShellOutput {
    pub fn is_pty(&self) -> bool {
        matches!(self, Self::Pty { .. })
    }

    fn validate(&self) -> Result<(), &'static str> {
        if let Self::Pty { screen } = self
            && (screen.cursor.x > screen.size.cols() || screen.cursor.y >= screen.size.rows())
        {
            return Err("shell cursor is outside its terminal");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShellRun {
    pub id: String,
    pub session_id: String,
    pub source_run_id: Option<String>,
    pub source_turn_id: String,
    pub source_tool_call_id: String,
    pub visibility: ShellVisibility,
    pub cwd: String,
    pub command: String,
    pub started_at: u64,
    pub updated_at: u64,
    pub timeout_ms: Option<u64>,
    pub revision: u64,
    pub state: ShellState,
    pub output: ShellOutput,
}

/// Only mutable facts may be patched. State and final output share one commit.
#[derive(Default)]
pub struct ShellPatch {
    pub state: Option<ShellState>,
    pub output: Option<ShellOutput>,
    pub observed_at: Option<u64>,
    pub updated_at: Option<u64>,
}

impl ShellRun {
    pub fn validate(&self) -> Result<(), &'static str> {
        crate::interaction::entity_id(&self.id)?;
        crate::interaction::entity_id(&self.session_id)?;
        crate::interaction::entity_id(&self.source_turn_id)?;
        if let Some(id) = &self.source_run_id {
            crate::interaction::entity_id(id)?;
        }
        if self.source_tool_call_id.is_empty()
            || self.source_tool_call_id.len() > 512
            || self.cwd.contains('\0')
            || self.command.contains('\0')
            || self.revision == 0
            || self.revision > MAX_SAFE
        {
            return Err("invalid shell resource metadata");
        }
        time(self.started_at)?;
        time(self.updated_at)?;
        if let Some(timeout) = self.timeout_ms {
            time(timeout)?;
        }
        self.state.validate()?;
        self.output.validate()?;
        Ok(())
    }

    pub fn patched(&self, patch: ShellPatch) -> Result<Self, &'static str> {
        let mut next = self.clone();
        if let Some(state) = patch.state {
            if !self.state.accepts(&state) {
                return Err("invalid shell state transition");
            }
            next.state = state;
        }
        if let Some(output) = patch.output {
            if self.output.is_pty() != output.is_pty() {
                return Err("shell output mode is immutable");
            }
            next.output = output;
        }
        if let Some(at) = patch.observed_at {
            time(at)?;
            match &mut next.state {
                ShellState::Terminal { observed_at, .. } => {
                    observed_at.get_or_insert(at);
                }
                _ => return Err("active shell cannot be observed as complete"),
            }
        }
        if let Some(at) = patch.updated_at {
            next.updated_at = at;
        }
        next.validate()?;
        if next != *self {
            next.revision = self
                .revision
                .checked_add(1)
                .filter(|r| *r <= MAX_SAFE)
                .ok_or("shell revision exhausted")?;
        }
        Ok(next)
    }
}

fn time(value: u64) -> Result<(), &'static str> {
    if value > MAX_SAFE {
        Err("invalid shell time")
    } else {
        Ok(())
    }
}
