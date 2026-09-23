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

use crate::{Client, ClientError, RequestFailure};
use maka_protocol::{
    Operation, ProtocolError,
    artifact::{
        self, ArtifactIngestInput as Ingest, ArtifactIngestResult as Ingested,
        ArtifactQueryInput as Query, ArtifactQueryResult as Queried,
    },
    turn::{AttachmentKind, StorageRef},
};

impl Client {
    /// Upload commands retain their caller-owned identity. No implicit retry or
    /// new upload ID is introduced after an uncertain response.
    pub async fn ingest_artifact(&self, input: Ingest) -> Result<Ingested, RequestFailure> {
        let value = self
            .request(
                Operation::ArtifactIngest,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output =
            artifact::decode_ingest_result(&value).map_err(|e| self.invalid_artifact(e))?;
        if !ingest_matches(&input, &output) {
            return Err(self.invalid_artifact(ProtocolError::invalid(
                "Artifact upload result does not match request",
            )));
        }
        Ok(output)
    }

    pub async fn query_artifact(&self, input: Query) -> Result<Queried, RequestFailure> {
        let value = self
            .request(
                Operation::ArtifactQuery,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let output = artifact::decode_query_result(&value).map_err(|e| self.invalid_artifact(e))?;
        if !query_matches(&input, &output) {
            return Err(self.invalid_artifact(ProtocolError::invalid(
                "Artifact query result does not match request",
            )));
        }
        Ok(output)
    }

    pub async fn delete_artifact(
        &self,
        input: artifact::ArtifactDeleteInput,
    ) -> Result<artifact::ArtifactDeleteResult, RequestFailure> {
        let value = self
            .request(
                Operation::ArtifactDelete,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        artifact::decode_delete_result(&value).map_err(|e| self.invalid_artifact(e))
    }

    fn invalid_artifact(&self, error: ProtocolError) -> RequestFailure {
        self.disconnect();
        RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
    }
}

fn ingest_matches(input: &Ingest, output: &Ingested) -> bool {
    let upload = match output {
        Ingested::UploadOpened { upload_id, .. }
        | Ingested::ChunkAccepted { upload_id, .. }
        | Ingested::UploadAborted { upload_id }
        | Ingested::Committed { upload_id, .. } => upload_id,
    };
    if upload != input.upload_id() {
        return false;
    }
    match (input, output) {
        (Ingest::Begin { total_bytes, .. }, Ingested::UploadOpened { next_offset, .. }) => {
            next_offset <= total_bytes
        }
        (
            Ingest::Chunk {
                offset,
                chunk_base64,
                ..
            },
            Ingested::ChunkAccepted { next_offset, .. },
        ) => {
            let Ok(bytes) = artifact::decode_chunk(chunk_base64, artifact::MAX_INGEST_CHUNK_BYTES)
            else {
                return false;
            };
            // Replaying an already accepted chunk reports the cumulative offset.
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= *next_offset)
                && *next_offset <= artifact::MAX_ATTACHMENT_BYTES
        }
        (Ingest::Abort { .. }, Ingested::UploadAborted { .. }) => true,
        (Ingest::Begin { .. } | Ingest::Commit { .. }, Ingested::Committed { attachment, .. }) => {
            matches!(&attachment.storage_ref, StorageRef::SessionFile { session_id, relative_path }
                if session_id == input.session_id() && *relative_path == artifact::upload_artifact_id(session_id, upload))
                && attachment.bytes <= artifact::MAX_ATTACHMENT_BYTES
                && attachment.name.len() <= 512
                && attachment.mime_type.len() <= 256
                && attachment.kind
                    == AttachmentKind::from_metadata(&attachment.mime_type, &attachment.name)
                && match input {
                    Ingest::Begin {
                        name,
                        mime_type,
                        total_bytes,
                        ..
                    } => {
                        attachment.name == artifact::normalize_name(name)
                            && attachment.mime_type == *mime_type
                            && attachment.bytes == *total_bytes
                    }
                    _ => true,
                }
        }
        _ => false,
    }
}

fn query_matches(input: &Query, output: &Queried) -> bool {
    if let Queried::RevisionChanged { expected, actual } = output {
        return matches!(input, Query::ListContinue { revision, .. } if revision == expected && actual != expected);
    }
    let session = match output {
        Queried::Artifact { session_id, .. }
        | Queried::Page { session_id, .. }
        | Queried::Text { session_id, .. }
        | Queried::Binary { session_id, .. }
        | Queried::Chunk { session_id, .. } => session_id,
        Queried::RevisionChanged { .. } => unreachable!(),
    };
    if session != input.session_id() {
        return false;
    }
    match (input, output) {
        (Query::Get { artifact_id, .. }, Queried::Artifact { artifact, .. }) => artifact
            .as_ref()
            .is_none_or(|item| item.id == *artifact_id && item.session_id == *session),
        (
            Query::ReadText { artifact_id, .. },
            Queried::Text {
                artifact_id: actual,
                ..
            },
        )
        | (
            Query::ReadBinary { artifact_id, .. },
            Queried::Binary {
                artifact_id: actual,
                ..
            },
        ) => artifact_id == actual,
        (
            Query::ReadChunk {
                artifact_id,
                offset,
                ..
            },
            Queried::Chunk {
                artifact_id: actual,
                offset: start,
                ..
            },
        ) => artifact_id == actual && offset == start,
        (
            Query::ListStart { .. } | Query::ListContinue { .. },
            Queried::Page {
                revision,
                artifacts,
                next_cursor,
                ..
            },
        ) => {
            let mut seen = std::collections::HashSet::new();
            artifacts
                .iter()
                .all(|item| item.session_id == *session && seen.insert(&item.id))
                && (next_cursor.is_none() || !artifacts.is_empty())
                && match input {
                    Query::ListContinue {
                        revision: expected,
                        cursor,
                        ..
                    } => revision == expected && next_cursor.as_ref() != Some(cursor),
                    _ => true,
                }
        }
        _ => false,
    }
}
