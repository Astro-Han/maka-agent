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

//! Invocation-scoped filesystem operations. Host supplies authority, not callers.
use maka_runtime::read::ReadInput;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "input",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Operation {
    Read(ReadInput),
    Write(Write),
    Edit(Edit),
    Glob(Glob),
    Grep(Grep),
    Patch(Patch),
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub path: String,
    pub content: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Glob {
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Grep {
    pub pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Patch {
    CreateFile { path: String, diff: String },
    UpdateFile { path: String, diff: String },
    DeleteFile { path: String },
}
impl Operation {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Read(_) => "Read",
            Self::Write(_) => "Write",
            Self::Edit(_) => "Edit",
            Self::Glob(_) => "Glob",
            Self::Grep(_) => "Grep",
            Self::Patch(_) => "apply_patch",
        }
    }
    /// Reuse the native tool contract; schema and file-effect semantics have one owner.
    pub fn into_tool_input(self, operation_id: &str) -> Value {
        match self {
            Self::Read(input) => json!(input),
            Self::Write(input) => json!(input),
            Self::Edit(input) => json!(input),
            Self::Glob(input) => json!(input),
            Self::Grep(input) => json!(input),
            Self::Patch(operation) => json!({"callId":operation_id,"operation":operation}),
        }
    }
}
