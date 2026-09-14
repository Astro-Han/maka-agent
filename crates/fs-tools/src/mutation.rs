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

use crate::failed;
use maka_runtime::tools::ToolError;
use serde::Deserialize;
use serde_json::{Value, json};

pub const WRITE_NAME: &str = "Write";
pub const WRITE_DESCRIPTION: &str = "Write complete UTF-8 content (at most 1 MiB) to a regular file within the Session's write roots. Parent directories must exist. Symlinks and parent (..) components are unsupported. Preserves existing inode and mode; not an atomic replacement.";
pub const EDIT_NAME: &str = "Edit";
pub const EDIT_DESCRIPTION: &str = "Replace one unique span in an existing UTF-8 regular file, trying exact, line-trimmed, whitespace-normalized, then escape-normalized matching. Ambiguous matches are rejected; new_string is written literally without reindentation. Fuzzy matching requires at least 5 trimmed UTF-16 units, text without NUL, at most 1,000,000 UTF-16 units and 50,000 lines, and a proportionate span. Source and output are limited to 1 MiB. Symlinks and parent (..) components are unsupported. Preserves existing inode and mode; not an atomic replacement.";
pub(crate) const MAX_CONTENT: usize = 1024 * 1024;
pub(crate) const MAX_PATH: usize = 4096;

pub fn write_schema() -> Value {
    json!({"type":"object","properties":{
        "path":{"type":"string","minLength":1,"maxLength":MAX_PATH},
        "content":{"type":"string","maxLength":MAX_CONTENT}
    },"required":["path","content"],"additionalProperties":false})
}

pub fn edit_schema() -> Value {
    json!({"type":"object","properties":{
        "path":{"type":"string","minLength":1,"maxLength":MAX_PATH},
        "old_string":{"type":"string","minLength":1,"maxLength":MAX_CONTENT},
        "new_string":{"type":"string","maxLength":MAX_CONTENT}
    },"required":["path","old_string","new_string"],"additionalProperties":false})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteInput {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditInput {
    path: String,
    old_string: String,
    new_string: String,
}

pub(crate) enum Mutation {
    Write(WriteInput),
    Edit(EditInput),
    PatchUpdate {
        path: String,
        chunks: Vec<apply_patch::UpdateChunk>,
    },
}

impl Mutation {
    pub(crate) fn parse(name: &str, input: Value) -> Result<Self, ToolError> {
        let mutation = match name {
            WRITE_NAME => {
                Self::Write(serde_json::from_value(input).map_err(|e| failed(e.to_string()))?)
            }
            EDIT_NAME => {
                Self::Edit(serde_json::from_value(input).map_err(|e| failed(e.to_string()))?)
            }
            _ => return Err(failed("unsupported tool")),
        };
        let bounded = match &mutation {
            Self::Write(input) => input.content.len() <= MAX_CONTENT,
            Self::Edit(input) => {
                !input.old_string.is_empty()
                    && input.old_string != input.new_string
                    && input.old_string.len() <= MAX_CONTENT
                    && input.new_string.len() <= MAX_CONTENT
            }
            Self::PatchUpdate { .. } => unreachable!("patch input is parsed separately"),
        };
        let path = mutation.path();
        if !bounded
            || path.is_empty()
            || path.len() > MAX_PATH
            || path.contains("://")
            || path.contains('\0')
        {
            return Err(failed(
                "mutation requires bounded file/text arguments; Edit old_string must be nonempty and different",
            ));
        }
        Ok(mutation)
    }

    pub(crate) fn path(&self) -> &str {
        match self {
            Self::Write(input) => &input.path,
            Self::Edit(input) => &input.path,
            Self::PatchUpdate { path, .. } => path,
        }
    }

    pub(crate) fn needs_read(&self) -> bool {
        matches!(self, Self::Edit(_) | Self::PatchUpdate { .. })
    }

    pub(crate) fn complete_write(path: String, content: String) -> Self {
        Self::Write(WriteInput { path, content })
    }

    pub(crate) fn prepare(
        self,
        file: Option<&mut cap_std::fs::File>,
        output_path: &str,
    ) -> Result<(String, Value), ToolError> {
        match self {
            Self::Write(input) => {
                let result =
                    json!({"kind":"file_write","path":output_path,"bytes":input.content.len()});
                Ok((input.content, result))
            }
            Self::Edit(input) => {
                let source = read_source(file)?;
                crate::edit_match::replace(
                    &source,
                    &input.old_string,
                    &input.new_string,
                    output_path,
                )
            }
            Self::PatchUpdate { chunks, .. } => {
                let source = read_source(file)?;
                let content = apply_patch::apply_update(&source, &chunks)
                    .map_err(|e| failed(e.to_string()))?;
                if content.len() > MAX_CONTENT {
                    return Err(failed("Patch output exceeds 1 MiB"));
                }
                Ok((content, json!({"status":"completed"})))
            }
        }
    }
}

fn read_source(file: Option<&mut cap_std::fs::File>) -> Result<String, ToolError> {
    use std::io::{Read, Seek, SeekFrom};
    let file = file.ok_or_else(|| failed("mutation requires an existing file"))?;
    if file.metadata().map_err(|e| failed(e.to_string()))?.len() > MAX_CONTENT as u64 {
        return Err(failed("mutation source exceeds 1 MiB"));
    }
    let mut bytes = Vec::new();
    (&mut *file)
        .take((MAX_CONTENT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| failed(e.to_string()))?;
    if bytes.len() > MAX_CONTENT {
        return Err(failed("mutation source exceeds 1 MiB"));
    }
    let source = String::from_utf8(bytes).map_err(|_| failed("mutation requires UTF-8 text"))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| failed(e.to_string()))?;
    Ok(source)
}
