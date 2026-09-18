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

use maka_plugins::{
    execution::{CreateChild, Submit},
    storage::Mutation,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
#[serde(tag = "method", content = "input", deny_unknown_fields)]
pub(super) enum Request {
    #[serde(rename = "credentials.read")]
    CredentialRead(Key),
    #[serde(rename = "credentials.write")]
    CredentialWrite(maka_plugins::credentials::Write),
    #[serde(rename = "terminal.spawn")]
    TerminalSpawn(super::super::terminal::Spawn),
    #[serde(rename = "terminal.control")]
    TerminalControl(super::super::terminal::Control),
    #[serde(rename = "terminal.next")]
    TerminalNext(ProcessHandle),
    #[serde(rename = "terminal.wait")]
    TerminalWait(ProcessHandle),
    #[serde(rename = "terminal.close")]
    TerminalClose(Handle),
    #[serde(rename = "files.invoke")]
    Files(super::super::effects::Request),
    #[serde(rename = "llm.generate")]
    Generate(super::super::effects::ModelRequest),
    #[serde(rename = "clients.tools")]
    ClientCatalog(Authority),
    #[serde(rename = "clients.call")]
    ClientCall(super::super::effects::ClientRequest),
    #[serde(rename = "http.request")]
    HttpSend(super::super::http::Request),
    #[serde(rename = "http.next")]
    HttpNext(ProcessHandle),
    #[serde(rename = "http.close")]
    HttpClose(Handle),
    #[serde(rename = "process.spawn")]
    ProcessSpawn(super::super::process::Spawn),
    #[serde(rename = "process.write")]
    ProcessWrite(ProcessWrite),
    #[serde(rename = "process.endInput")]
    ProcessEndInput(ProcessHandle),
    #[serde(rename = "process.next")]
    ProcessNext(ProcessHandle),
    #[serde(rename = "process.wait")]
    ProcessWait(ProcessHandle),
    #[serde(rename = "process.close")]
    ProcessClose(Handle),
    #[serde(rename = "executor.emit")]
    ExecutorEmit(ExecutorOutput),
    #[serde(rename = "contribution.publish")]
    Publish(Vec<super::super::registration::Registration>),
    #[serde(rename = "contribution.release")]
    Unpublish(Handle),
    #[serde(rename = "contribution.withdraw")]
    Withdraw(Withdraw),
    #[serde(rename = "storage.read")]
    Read(Key),
    #[serde(rename = "storage.batch")]
    Batch(Batch),
    #[serde(rename = "execution.submit")]
    Submit(Submit),
    #[serde(rename = "execution.createChild")]
    CreateChild(CreateChild),
    #[serde(rename = "execution.workspacePatch")]
    WorkspacePatch(Operation),
    #[serde(rename = "execution.query")]
    Query(Operation),
    #[serde(rename = "execution.cancel")]
    Cancel(Operation),
    #[serde(rename = "execution.events")]
    Events(Events),
    #[serde(rename = "execution.event")]
    Event(Event),
    #[serde(rename = "service.provide")]
    Provide(Provide),
    #[serde(rename = "service.get")]
    Get(Name),
    #[serde(rename = "service.call")]
    Call(Call),
    #[serde(rename = "service.release")]
    Release(Handle),
    #[serde(rename = "clock.sleep")]
    Sleep(Sleep),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessHandle {
    pub authority: String,
    pub handle: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Authority {
    pub authority: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessWrite {
    #[serde(flatten)]
    pub target: ProcessHandle,
    pub bytes: Vec<u8>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecutorOutput {
    pub handle: String,
    pub output: maka_runtime::executor::Output,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Withdraw {
    pub kind: super::super::registration::Kind,
    pub name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Key {
    pub key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Batch {
    pub mutations: Vec<Mutation>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Operation {
    pub operation_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Events {
    pub operation_id: String,
    pub after: u64,
    pub through: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Event {
    pub operation_id: String,
    pub event_id: String,
    pub through: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Provide {
    pub name: String,
    pub callback: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Name {
    pub name: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Call {
    pub handle: String,
    pub input: Value,
    pub authority: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Handle {
    pub handle: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Sleep {
    pub milliseconds: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Code {
    Invalid,
    Revoked,
    Conflict,
    OutcomeUnknown,
    Unavailable,
    Busy,
    NotFound,
}
#[derive(Serialize)]
pub(super) struct Error {
    pub code: Code,
    pub message: String,
}
impl Error {
    pub fn tool(error: maka_runtime::tools::ToolError) -> Self {
        Self {
            code: match error {
                maka_runtime::tools::ToolError::Failed(_) => Code::Invalid,
                _ => Code::OutcomeUnknown,
            },
            message: error.to_string(),
        }
    }
    pub fn invalid(error: impl ToString) -> Self {
        Self {
            code: Code::Invalid,
            message: error.to_string(),
        }
    }
}
impl From<maka_plugins::Error> for Error {
    fn from(error: maka_plugins::Error) -> Self {
        Self {
            code: if error == maka_plugins::Error::Retired {
                Code::Revoked
            } else {
                Code::Invalid
            },
            message: error.to_string(),
        }
    }
}
impl From<maka_plugins::storage::StoreError> for Error {
    fn from(error: maka_plugins::storage::StoreError) -> Self {
        use maka_plugins::storage::StoreError;
        let code = match &error {
            StoreError::Retired => Code::Revoked,
            StoreError::Conflict { .. } => Code::Conflict,
            StoreError::OutcomeUnknown(_) => Code::OutcomeUnknown,
            StoreError::Unavailable(_) => Code::Unavailable,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}
impl From<maka_plugins::execution::CommandError> for Error {
    fn from(error: maka_plugins::execution::CommandError) -> Self {
        use maka_plugins::execution::CommandError;
        let code = match &error {
            CommandError::Revoked | CommandError::Denied => Code::Revoked,
            CommandError::Conflict => Code::Conflict,
            CommandError::NotFound => Code::NotFound,
            CommandError::Busy => Code::Busy,
            CommandError::OutcomeUnknown(_) => Code::OutcomeUnknown,
            CommandError::Invalid(_) => Code::Invalid,
            CommandError::Draining | CommandError::Host(_) => Code::Unavailable,
        };
        Self {
            code,
            message: error.to_string(),
        }
    }
}
