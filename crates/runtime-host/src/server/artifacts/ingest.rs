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

use super::{Code, Host, OperationError, error, staging::Manifest, store_error};
use maka_protocol::artifact::{
    ArtifactIngestInput as Input, ArtifactIngestResult as Output, MAX_INGEST_CHUNK_BYTES,
    decode_chunk,
};
use maka_runtime::{
    artifact::{Artifact, ArtifactKind, ArtifactSource, normalize_name, upload_artifact_id},
    attachment::{AttachmentKind, AttachmentRef, StorageRef},
};

pub(super) async fn execute(
    host: &Host,
    owner: uuid::Uuid,
    input: Input,
) -> Result<Output, OperationError> {
    let session_id = input.session_id().to_owned();
    let upload_id = input.upload_id().to_owned();
    let key = (session_id.clone(), upload_id.clone());
    // This also verifies Session existence before any staging mutation.
    let committed = host
        .log
        .get_artifact(&session_id, &upload_artifact_id(&session_id, &upload_id))
        .await
        .map_err(store_error)?
        .record;
    match input {
        Input::Begin {
            name,
            mime_type,
            total_bytes,
            content_sha256,
            ..
        } => {
            let manifest = Manifest {
                name: normalize_name(&name),
                mime_type,
                total_bytes,
                digest: content_sha256,
            };
            if let Some(record) = committed {
                if record.name != manifest.name
                    || record.mime_type.as_ref() != Some(&manifest.mime_type)
                    || record.size_bytes != total_bytes
                    || record.summary.as_ref() != Some(&manifest.digest)
                    || record.kind != artifact_kind(&manifest)
                {
                    return Err(error(
                        Code::OperationConflict,
                        "Upload identity was already committed",
                    ));
                }
                return receipt(upload_id, record);
            }
            let next_offset = host.uploads.open(key, owner, manifest)?;
            Ok(Output::UploadOpened {
                upload_id,
                next_offset,
            })
        }
        Input::Chunk {
            offset,
            chunk_base64,
            ..
        } => {
            let bytes = decode_chunk(&chunk_base64, MAX_INGEST_CHUNK_BYTES)
                .map_err(|e| error(Code::InvalidRequest, &e.message))?;
            let next_offset = host.uploads.accept(&key, owner, offset, &bytes)?;
            Ok(Output::ChunkAccepted {
                upload_id,
                next_offset,
            })
        }
        Input::Abort { .. } => {
            host.uploads.abort(&key, owner)?;
            Ok(Output::UploadAborted { upload_id })
        }
        Input::Commit { .. } => {
            if let Some(record) = committed {
                return receipt(upload_id, record);
            }
            let upload = host.uploads.consume(&key, owner)?;
            let manifest = &upload.manifest;
            let artifact = Artifact {
                id: upload_artifact_id(&session_id, &upload_id),
                session_id,
                turn_id: upload_id.clone(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| error(Code::InternalFailure, "Invalid clock"))?
                    .as_millis() as u64,
                name: manifest.name.clone(),
                kind: artifact_kind(manifest),
                size_bytes: manifest.total_bytes,
                mime_type: Some(manifest.mime_type.clone()),
                source: ArtifactSource::UserUpload,
                summary: Some(manifest.digest.clone()),
            };
            // The independent store job checks SHA before insertion and retains
            // this buffer's byte permit through commit, even if the caller drops.
            let record = host
                .log
                .commit_artifact(artifact, upload)
                .await
                .map_err(store_error)?;
            receipt(upload_id, record)
        }
    }
}
fn artifact_kind(manifest: &Manifest) -> ArtifactKind {
    match AttachmentKind::from_metadata(&manifest.mime_type, &manifest.name) {
        AttachmentKind::Image => ArtifactKind::Image,
        AttachmentKind::Pdf => ArtifactKind::Pdf,
        _ => ArtifactKind::File,
    }
}
fn receipt(upload_id: String, record: Artifact) -> Result<Output, OperationError> {
    if record.source != ArtifactSource::UserUpload || record.turn_id != upload_id {
        return Err(error(
            Code::OperationConflict,
            "Upload identity belongs to another Artifact",
        ));
    }
    let mime_type = record.mime_type.ok_or_else(|| {
        error(
            Code::PersistenceFailed,
            "Committed attachment has no media type",
        )
    })?;
    Ok(Output::Committed {
        upload_id,
        attachment: AttachmentRef {
            kind: AttachmentKind::from_metadata(&mime_type, &record.name),
            name: record.name,
            mime_type,
            bytes: record.size_bytes,
            storage_ref: StorageRef::SessionFile {
                session_id: record.session_id,
                relative_path: record.id,
            },
        },
    })
}
