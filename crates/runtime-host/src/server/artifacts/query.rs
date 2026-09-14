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

use super::{Code, Host, OperationError, error, preview, store_error};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use maka_event_log::artifacts::ArtifactPage;
use maka_protocol::artifact::*;

pub(super) async fn execute(
    host: &Host,
    input: ArtifactQueryInput,
) -> Result<ArtifactQueryResult, OperationError> {
    use ArtifactQueryInput as Input;
    use ArtifactQueryResult as Output;
    let session_id = input.session_id().to_owned();
    match input {
        Input::ListStart { .. } => {
            let page = host
                .log
                .list_artifacts(&session_id, 0, MAX_PAGE_ITEMS)
                .await
                .map_err(store_error)?;
            page_result(session_id, page, 0)
        }
        Input::ListContinue {
            revision, cursor, ..
        } => {
            let offset = cursor
                .parse::<u64>()
                .ok()
                .filter(|n| *n <= 9_007_199_254_740_991 && n.to_string() == cursor);
            let page = host
                .log
                .list_artifacts(&session_id, offset.unwrap_or(0), MAX_PAGE_ITEMS)
                .await
                .map_err(store_error)?;
            if page.revision != revision {
                return Ok(Output::RevisionChanged {
                    expected: revision,
                    actual: page.revision,
                });
            }
            let offset = offset
                .filter(|n| *n > 0 && *n < page.total)
                .ok_or_else(|| error(Code::InvalidRequest, "Artifact cursor is invalid"))?;
            page_result(session_id, page, offset)
        }
        Input::Get { artifact_id, .. } => {
            let entry = host
                .log
                .get_artifact(&session_id, &artifact_id)
                .await
                .map_err(store_error)?;
            Ok(Output::Artifact {
                session_id,
                revision: entry.revision,
                artifact: entry.record.map(preview::project),
            })
        }
        Input::ReadText { artifact_id, .. } => {
            let chunk = host
                .log
                .read_artifact_chunk(&session_id, &artifact_id, 0, MAX_PREVIEW_BYTES)
                .await
                .map_err(store_error)?;
            let mut result = Output::Text {
                session_id,
                artifact_id,
                preview: preview::text(chunk),
            };
            if result_limit(&result).is_err() {
                let Output::Text { preview, .. } = &mut result else {
                    unreachable!()
                };
                *preview = Err(ReadFailure::TooLarge);
            }
            Ok(result)
        }
        Input::ReadBinary { artifact_id, .. } => {
            let chunk = host
                .log
                .read_artifact_chunk(&session_id, &artifact_id, 0, MAX_PREVIEW_BYTES)
                .await
                .map_err(store_error)?;
            Ok(Output::Binary {
                session_id,
                artifact_id,
                preview: preview::binary(chunk),
            })
        }
        Input::ReadChunk {
            artifact_id,
            offset,
            ..
        } => {
            let chunk = host
                .log
                .read_artifact_chunk(&session_id, &artifact_id, offset, MAX_READ_CHUNK_BYTES)
                .await
                .map_err(store_error)?
                .ok_or_else(|| error(Code::NotFound, "Artifact was not found"))?;
            let next = offset + chunk.bytes.len() as u64;
            Ok(Output::Chunk {
                session_id,
                artifact_id,
                offset,
                total_bytes: chunk.total_bytes,
                chunk_base64: STANDARD.encode(chunk.bytes),
                next_offset: (next < chunk.total_bytes).then_some(next),
            })
        }
    }
}

fn page_result(
    session_id: String,
    page: ArtifactPage,
    offset: u64,
) -> Result<ArtifactQueryResult, OperationError> {
    let mut output = ArtifactQueryResult::Page {
        session_id,
        revision: page.revision,
        artifacts: Vec::new(),
        next_cursor: None,
    };
    for record in page.records {
        if let ArtifactQueryResult::Page {
            artifacts,
            next_cursor,
            ..
        } = &mut output
        {
            artifacts.push(preview::project(record));
            let next = offset + artifacts.len() as u64;
            *next_cursor = (next < page.total).then(|| next.to_string());
        }
        if result_limit(&output).is_err() {
            if let ArtifactQueryResult::Page {
                artifacts,
                next_cursor,
                ..
            } = &mut output
            {
                artifacts.pop();
                if artifacts.is_empty() {
                    return Err(error(
                        Code::PersistenceFailed,
                        "Artifact cannot fit in one page",
                    ));
                }
                *next_cursor = Some((offset + artifacts.len() as u64).to_string());
            }
            break;
        }
    }
    Ok(output)
}
