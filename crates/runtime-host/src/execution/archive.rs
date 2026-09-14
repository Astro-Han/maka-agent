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

use maka_event_log::{
    EventLog, StoreError,
    archive::{ArchiveError, ToolResultResource},
};
use maka_runtime::{
    archive::{MAX_ARCHIVE_BYTES, ToolResultAddress},
    read::ReadRequest,
    tool_output::ToolSuccess,
    tools::ToolError,
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ReadFailure {
    NotFound,
    TooLarge,
    SourceMismatch,
    SizeMismatch,
    Corrupt,
    ReadFailed,
}

#[derive(Serialize)]
struct Unavailable {
    error: ReadFailure,
    message: &'static str,
}

pub(super) async fn read(
    log: &EventLog,
    session: &str,
    address: ToolResultAddress,
    request: &ReadRequest,
    cancellation: &CancellationToken,
) -> Result<ToolSuccess, ToolError> {
    let result = match address {
        ToolResultAddress::Event(event) => log.read_tool_result(session, &event).await,
        ToolResultAddress::Evidence(identity) => {
            if identity.original_bytes > MAX_ARCHIVE_BYTES as u64 {
                return Ok(unavailable(ReadFailure::TooLarge));
            }
            log.read_archive(session, &identity)
                .await
                .and_then(|bytes| {
                    bytes
                        .map(|bytes| {
                            String::from_utf8(bytes)
                                .map(|serialized_result| ToolResultResource {
                                    tool_name: identity.tool_name,
                                    serialized_result,
                                })
                                .map_err(|_| ArchiveError::Corrupt.into())
                        })
                        .transpose()
                })
        }
    };
    if cancellation.is_cancelled() {
        return Err(ToolError::Failed("Read cancelled".into()));
    }
    let resource = match result {
        Ok(Some(resource)) => resource,
        Ok(None) => return Ok(unavailable(ReadFailure::NotFound)),
        Err(error) => {
            return Ok(unavailable(match error {
                StoreError::Archive(ArchiveError::SourceMismatch) => ReadFailure::SourceMismatch,
                StoreError::Archive(ArchiveError::SizeMismatch) => ReadFailure::SizeMismatch,
                StoreError::Archive(ArchiveError::Corrupt) | StoreError::InvalidTransition(_) => {
                    ReadFailure::Corrupt
                }
                _ => ReadFailure::ReadFailed,
            }));
        }
    };
    let page = request
        .tool_result_page(&resource.tool_name, &resource.serialized_result)
        .map_err(|error| ToolError::Failed(error.to_string()))?;
    Ok(serde_json::to_value(page)
        .expect("typed page is JSON")
        .into())
}

fn unavailable(error: ReadFailure) -> ToolSuccess {
    serde_json::to_value(Unavailable {
        error,
        message: "This Maka content could not be read. Changing offset or limit will not restore it. Use the original source if it is still available.",
    }).expect("typed read failure is JSON").into()
}
