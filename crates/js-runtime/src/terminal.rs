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

use crate::trusted::TrustedRuntime;
use maka_runtime::terminal::{TerminalScreen, TerminalSize};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::oneshot;

const MAX_WRITE_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("terminal screen failed: {0}")]
pub struct ScreenError(String);

/// Replies and screen belong to the same completed parser cut.
#[derive(Debug, Deserialize)]
pub struct ScreenCut {
    pub replies: String,
    pub screen: TerminalScreen,
}

/// One terminal's serial parser state, hosted by a shared trusted JS worker.
/// A dropped/failed cut poisons only this handle, never another terminal/model.
pub struct Screen {
    runtime: TrustedRuntime,
    id: Option<u32>,
    available: bool,
}

pub(super) enum Operation {
    Write(String),
    Resize(TerminalSize),
    ResetAfterGap,
    Snapshot,
}

impl Operation {
    pub fn arguments(self) -> (&'static str, Value) {
        match self {
            Self::Write(data) => ("write", json!(data)),
            Self::Resize(size) => ("resize", json!(size)),
            Self::ResetAfterGap => ("resetAfterGap", Value::Null),
            Self::Snapshot => ("snapshot", Value::Null),
        }
    }
}

impl Screen {
    pub fn new(size: TerminalSize) -> Result<Self, ScreenError> {
        Self::with_runtime(TrustedRuntime::default(), size)
    }

    pub fn with_runtime(runtime: TrustedRuntime, size: TerminalSize) -> Result<Self, ScreenError> {
        let id = runtime.create_terminal(json!(size)).map_err(failure)?;
        Ok(Self {
            runtime,
            id: Some(id),
            available: true,
        })
    }

    pub async fn write(&mut self, data: &str) -> Result<ScreenCut, ScreenError> {
        if data.len() > MAX_WRITE_BYTES {
            return Err(ScreenError("write exceeds 64 KiB".into()));
        }
        self.call(Operation::Write(data.into())).await
    }

    pub async fn resize(&mut self, size: TerminalSize) -> Result<TerminalScreen, ScreenError> {
        self.call(Operation::Resize(size)).await
    }

    pub async fn reset_after_gap(&mut self) -> Result<(), ScreenError> {
        self.call(Operation::ResetAfterGap).await
    }

    pub async fn snapshot(&mut self) -> Result<TerminalScreen, ScreenError> {
        self.call(Operation::Snapshot).await
    }

    pub async fn close(mut self) {
        if let Some(id) = self.id.take() {
            let (reply, done) = oneshot::channel();
            self.runtime.dispose(id, Some(reply));
            let _ = done.await;
        }
    }

    async fn call<T: DeserializeOwned>(&mut self, operation: Operation) -> Result<T, ScreenError> {
        if !self.available {
            return Err(ScreenError(
                "parser is closed after an incomplete or failed operation".into(),
            ));
        }
        self.available = false;
        let json = self
            .runtime
            .terminal(self.id.expect("open screen"), operation)
            .await
            .map_err(failure)?;
        let result = serde_json::from_str(&json).map_err(failure)?;
        self.available = true;
        Ok(result)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.runtime.dispose(id, None);
        }
    }
}

fn failure(error: impl std::fmt::Display) -> ScreenError {
    ScreenError(error.to_string())
}
