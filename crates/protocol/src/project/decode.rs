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

use super::*;
use crate::{ProtocolError, Result, codec};
use serde_json::{Map, Value};

pub fn decode_query(value: &Value) -> Result<Query> {
    let map = codec::record(value, "project query")?;
    Ok(match value["kind"].as_str() {
        Some("list_start") => {
            codec::exact(map, &["kind", "view"])?;
            Query::ListStart {
                view: view(&value["view"])?,
            }
        }
        Some("list_continue") => {
            codec::exact(map, &["kind", "view", "revision", "cursor"])?;
            Query::ListContinue {
                view: view(&value["view"])?,
                revision: revision(&value["revision"])?,
                cursor: text(&value["cursor"], 128)?,
            }
        }
        Some("directory_roots") => {
            codec::exact(map, &["kind"])?;
            Query::DirectoryRoots
        }
        Some("directory_list_start") => {
            codec::exact(map, &["kind", "rootId", "segments"])?;
            Query::DirectoryListStart {
                root_id: id(&value["rootId"])?,
                segments: segments(&value["segments"])?,
            }
        }
        Some("directory_list_continue") => {
            codec::exact(map, &["kind", "rootId", "segments", "cursor"])?;
            Query::DirectoryListContinue {
                root_id: id(&value["rootId"])?,
                segments: segments(&value["segments"])?,
                cursor: segment(&value["cursor"])?,
            }
        }
        _ => return Err(invalid()),
    })
}

pub fn decode_mutation(value: &Value) -> Result<Mutation> {
    let map = codec::record(value, "project mutation")?;
    Ok(match value["kind"].as_str() {
        Some("register") => {
            codec::shaped(map, &["kind", "path"], &["prefer"])?;
            Mutation::Register {
                path: path(&value["path"])?,
                prefer: map.get("prefer").map(boolean).transpose()?,
            }
        }
        Some("register_directory") => {
            codec::exact(map, &["kind", "rootId", "segments"])?;
            Mutation::RegisterDirectory {
                root_id: id(&value["rootId"])?,
                segments: segments(&value["segments"])?,
            }
        }
        Some("relink") => {
            codec::exact(map, &["kind", "projectId", "path"])?;
            Mutation::Relink {
                project_id: id(&value["projectId"])?,
                path: path(&value["path"])?,
            }
        }
        Some("rename") => {
            codec::exact(map, &["kind", "projectId", "name"])?;
            Mutation::Rename {
                project_id: id(&value["projectId"])?,
                name: text(&value["name"], 16 * 1024)?,
            }
        }
        Some("archive" | "restore") => {
            codec::exact(map, &["kind", "projectId"])?;
            let project_id = id(&value["projectId"])?;
            if value["kind"] == "archive" {
                Mutation::Archive { project_id }
            } else {
                Mutation::Restore { project_id }
            }
        }
        _ => return Err(invalid()),
    })
}

pub(super) fn exact<'a>(value: &'a Value, keys: &[&str]) -> Result<&'a Map<String, Value>> {
    let map = codec::record(value, "project value")?;
    codec::exact(map, keys)?;
    Ok(map)
}

pub(super) fn view(value: &Value) -> Result<View> {
    match value.as_str() {
        Some("summary") => Ok(View::Summary),
        Some("locations") => Ok(View::Locations),
        _ => Err(invalid()),
    }
}

pub(super) fn text(value: &Value, max: usize) -> Result<String> {
    value
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= max)
        .map(str::to_owned)
        .ok_or_else(invalid)
}

pub(super) fn id(value: &Value) -> Result<String> {
    let text = text(value, 128)?;
    crate::turn::entity(&text)?;
    Ok(text)
}

pub(super) fn path(value: &Value) -> Result<String> {
    let text = text(value, 4096)?;
    if !crate::codec::absolute_host_path(&text) {
        return Err(invalid());
    }
    Ok(text)
}

pub(super) fn revision(value: &Value) -> Result<String> {
    let text = text(value, 71)?;
    if text.len() != 71
        || !text.starts_with("sha256:")
        || !text[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    Ok(text)
}

pub(super) fn segment(value: &Value) -> Result<String> {
    let text = text(value, 255)?;
    if matches!(text.as_str(), "." | "..") || text.contains(['/', '\\']) {
        return Err(invalid());
    }
    Ok(text)
}

pub(super) fn segments(value: &Value) -> Result<Vec<String>> {
    array(value, DIRECTORY_MAX_SEGMENTS)?
        .iter()
        .map(segment)
        .collect()
}

pub(super) fn array(value: &Value, max: usize) -> Result<&Vec<Value>> {
    value
        .as_array()
        .filter(|items| items.len() <= max)
        .ok_or_else(invalid)
}

pub(super) fn boolean(value: &Value) -> Result<bool> {
    value.as_bool().ok_or_else(invalid)
}

pub(super) fn count(value: &Value) -> Result<u64> {
    codec::count(value, "project count")
}

pub(super) fn nullable<T>(
    value: &Value,
    decode: impl FnOnce(&Value) -> Result<T>,
) -> Result<Option<T>> {
    if value.is_null() {
        Ok(None)
    } else {
        decode(value).map(Some)
    }
}

pub(super) fn invalid() -> ProtocolError {
    ProtocolError::invalid("Invalid Project contract")
}
