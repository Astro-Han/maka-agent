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

use base64::{Engine as _, engine::general_purpose::STANDARD};
use maka_client::{Client, ClientError, RequestFailure};
use maka_protocol::{
    Operation, OperationErrorCode as Code,
    artifact::{
        ArtifactDeleteInput, ArtifactIngestInput as Ingest, ArtifactIngestResult as Ingested,
        ArtifactQueryInput as Query, ArtifactQueryResult as Queried, BinaryReadFailure,
        ReadFailure,
    },
    turn::{AttachmentRef, StorageRef},
};
use maka_runtime::artifact::{Artifact, content_digest, upload_artifact_id};
use serde_json::json;
use std::{path::PathBuf, time::Duration};

pub(super) struct Endpoint {
    pub path: PathBuf,
    pub root: String,
    pub epoch: String,
}
impl Endpoint {
    pub async fn open(
        &self,
    ) -> (
        Client,
        tokio::sync::mpsc::Receiver<maka_client::Notification>,
    ) {
        Client::connect(
            maka_client::local::open_stream(&self.path).await.unwrap(),
            &self.root,
            &self.epoch,
            maka_client::Operations,
        )
        .await
        .unwrap()
    }
}
pub(super) async fn initialize(client: &Client, workspace: &std::path::Path) {
    let created = super::super::support::model_connection::create(
        client,
        "openai-compatible",
        "unused",
        "http://127.0.0.1:1/v1",
        "unused-fixture",
        json!({"fixture-model":{}}),
    )
    .await;
    client.request(Operation::ConnectionCatalogSetDefaultTarget,json!({
        "expectedCatalogRevision":created["catalogRevision"],"target":{"connectionId":created["connection"]["connectionId"],"modelId":"fixture-model"}
    })).await.unwrap();
    for session in ["session", "other"] {
        client.create_session(maka_protocol::session::decode_session_create_input(&json!({
            "sessionId":session,"workspace":{"kind":"host_path","path":workspace},"modelTarget":{"kind":"default"}
        })).unwrap()).await.unwrap();
    }
}
fn begin(id: &str, bytes: &[u8]) -> Ingest {
    Ingest::Begin {
        session_id: "session".into(),
        upload_id: id.into(),
        name: " ../file?.txt ".into(),
        mime_type: "text/plain".into(),
        total_bytes: bytes.len() as u64,
        content_sha256: content_digest(bytes),
    }
}
fn chunk(id: &str, offset: usize, bytes: &[u8]) -> Ingest {
    Ingest::Chunk {
        session_id: "session".into(),
        upload_id: id.into(),
        offset: offset as u64,
        chunk_base64: STANDARD.encode(bytes),
    }
}
fn commit(id: &str) -> Ingest {
    Ingest::Commit {
        session_id: "session".into(),
        upload_id: id.into(),
    }
}
fn abort(id: &str) -> Ingest {
    Ingest::Abort {
        session_id: "session".into(),
        upload_id: id.into(),
    }
}
fn rejected<T: std::fmt::Debug>(result: Result<T, RequestFailure>, expected: Code) {
    assert!(
        matches!(&result,Err(RequestFailure::Rejected(ClientError::Rejected(error))) if error.code==expected),
        "{result:?}"
    );
}
fn attachment(result: Ingested) -> AttachmentRef {
    let Ingested::Committed { attachment, .. } = result else {
        panic!("Expected commit: {result:?}")
    };
    attachment
}
fn id(file: &AttachmentRef) -> String {
    let StorageRef::SessionFile { relative_path, .. } = &file.storage_ref else {
        panic!()
    };
    relative_path.clone()
}
async fn upload(client: &Client, upload: &str, bytes: &[u8]) -> AttachmentRef {
    client.ingest_artifact(begin(upload, bytes)).await.unwrap();
    for (index, part) in bytes.chunks(48 * 1024).enumerate() {
        client
            .ingest_artifact(chunk(upload, index * 48 * 1024, part))
            .await
            .unwrap();
    }
    attachment(client.ingest_artifact(commit(upload)).await.unwrap())
}
async fn get(client: &Client, artifact_id: &str) -> Option<Artifact> {
    let Queried::Artifact { artifact, .. } = client
        .query_artifact(Query::Get {
            session_id: "session".into(),
            artifact_id: artifact_id.into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    artifact
}
async fn read_all(client: &Client, artifact_id: &str) -> Vec<u8> {
    let mut bytes = vec![];
    let mut offset = 0;
    loop {
        let Queried::Chunk {
            chunk_base64,
            next_offset,
            ..
        } = client
            .query_artifact(Query::ReadChunk {
                session_id: "session".into(),
                artifact_id: artifact_id.into(),
                offset,
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        bytes.extend(STANDARD.decode(chunk_base64).unwrap());
        match next_offset {
            Some(next) => offset = next,
            None => return bytes,
        }
    }
}
async fn page(client: &Client) -> (String, Vec<Artifact>) {
    let mut query = Query::ListStart {
        session_id: "session".into(),
    };
    let mut all = vec![];
    loop {
        let Queried::Page {
            revision,
            artifacts,
            next_cursor,
            ..
        } = client.query_artifact(query).await.unwrap()
        else {
            panic!()
        };
        all.extend(artifacts);
        match next_cursor {
            Some(cursor) => {
                query = Query::ListContinue {
                    session_id: "session".into(),
                    revision,
                    cursor,
                }
            }
            None => return (revision, all),
        }
    }
}
async fn delete(
    client: &Client,
    session: &str,
    artifact: &str,
) -> Result<maka_protocol::artifact::ArtifactDeleteResult, RequestFailure> {
    client
        .delete_artifact(ArtifactDeleteInput {
            session_id: session.into(),
            artifact_id: artifact.into(),
        })
        .await
}
pub(super) async fn verify(client: &Client, endpoint: &Endpoint, reopened: bool) {
    let bytes = "Maka 😀\n".repeat(9000).into_bytes();
    let original = begin("durable-upload", &bytes);
    let artifact_id = upload_artifact_id("session", "durable-upload");
    if reopened {
        let receipt = client.ingest_artifact(original).await.unwrap();
        assert_eq!(id(&attachment(receipt.clone())), artifact_id);
        assert_eq!(
            client
                .ingest_artifact(commit("durable-upload"))
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(read_all(client, &artifact_id).await, bytes);
        assert_eq!(page(client).await.1.len(), 132);
        rejected(
            delete(client, "session", "protected-evidence").await,
            Code::OperationConflict,
        );
        rejected(
            client.ingest_artifact(commit("uncommitted")).await,
            Code::NotFound,
        );
        return;
    }
    let (sibling, _sibling_notices) = endpoint.open().await;
    assert!(matches!(
        client.ingest_artifact(original.clone()).await.unwrap(),
        Ingested::UploadOpened { next_offset: 0, .. }
    ));
    assert!(page(client).await.1.is_empty());
    assert!(get(client, &artifact_id).await.is_none());
    rejected(
        sibling.ingest_artifact(original.clone()).await,
        Code::OperationConflict,
    );
    rejected(
        sibling.ingest_artifact(commit("durable-upload")).await,
        Code::NotFound,
    );
    let first = chunk("durable-upload", 0, &bytes[..48 * 1024]);
    for _ in 0..2 {
        assert!(matches!(
            client.ingest_artifact(first.clone()).await.unwrap(),
            Ingested::ChunkAccepted {
                next_offset: 49152,
                ..
            }
        ));
    }
    assert!(matches!(
        client.ingest_artifact(original.clone()).await.unwrap(),
        Ingested::UploadOpened {
            next_offset: 49152,
            ..
        }
    ));
    sibling
        .ingest_artifact(abort("durable-upload"))
        .await
        .unwrap();
    rejected(
        client
            .ingest_artifact(chunk("durable-upload", 1, &bytes[..48 * 1024]))
            .await,
        Code::OperationConflict,
    );
    rejected(
        client.ingest_artifact(commit("durable-upload")).await,
        Code::OperationConflict,
    );
    for offset in (48 * 1024..bytes.len()).step_by(48 * 1024) {
        client
            .ingest_artifact(chunk(
                "durable-upload",
                offset,
                &bytes[offset..(offset + 48 * 1024).min(bytes.len())],
            ))
            .await
            .unwrap();
    }
    // A replay can acknowledge more than the end of the replayed chunk.
    assert!(
        matches!(client.ingest_artifact(first).await.unwrap(),Ingested::ChunkAccepted {next_offset,..} if next_offset==bytes.len() as u64)
    );
    let receipt = client
        .ingest_artifact(commit("durable-upload"))
        .await
        .unwrap();
    let file = attachment(receipt.clone());
    assert_eq!(
        (file.name.as_str(), file.mime_type.as_str(), file.bytes),
        ("file-.txt", "text/plain", bytes.len() as u64)
    );
    assert_eq!(id(&file), artifact_id);
    assert_eq!(
        sibling.ingest_artifact(original.clone()).await.unwrap(),
        receipt
    );
    assert_eq!(
        sibling
            .ingest_artifact(commit("durable-upload"))
            .await
            .unwrap(),
        receipt
    );
    let mut changed = original;
    if let Ingest::Begin { content_sha256, .. } = &mut changed {
        *content_sha256 = content_digest(b"other");
    }
    rejected(
        sibling.ingest_artifact(changed).await,
        Code::OperationConflict,
    );
    assert_eq!(read_all(client, &artifact_id).await, bytes);
    let eof = Query::ReadChunk {
        session_id: "session".into(),
        artifact_id: artifact_id.clone(),
        offset: bytes.len() as u64,
    };
    assert!(
        matches!(client.query_artifact(eof.clone()).await.unwrap(),Queried::Chunk {chunk_base64,next_offset:None,..} if chunk_base64.is_empty())
    );
    let mut beyond = eof;
    if let Query::ReadChunk { offset, .. } = &mut beyond {
        *offset += 1;
    }
    rejected(client.query_artifact(beyond).await, Code::InvalidRequest);
    assert!(matches!(
        client
            .query_artifact(Query::ReadText {
                session_id: "session".into(),
                artifact_id: artifact_id.clone()
            })
            .await
            .unwrap(),
        Queried::Text {
            preview: Err(ReadFailure::TooLarge),
            ..
        }
    ));
    assert!(matches!(
        client
            .query_artifact(Query::Get {
                session_id: "other".into(),
                artifact_id: artifact_id.clone()
            })
            .await
            .unwrap(),
        Queried::Artifact { artifact: None, .. }
    ));
    rejected(delete(client, "other", &artifact_id).await, Code::NotFound);

    let mut wrong = begin("digest-mismatch", b"bad");
    if let Ingest::Begin { content_sha256, .. } = &mut wrong {
        *content_sha256 = content_digest(b"yes");
    }
    client.ingest_artifact(wrong).await.unwrap();
    client
        .ingest_artifact(chunk("digest-mismatch", 0, b"bad"))
        .await
        .unwrap();
    rejected(
        client.ingest_artifact(commit("digest-mismatch")).await,
        Code::OperationConflict,
    );
    rejected(
        client.ingest_artifact(commit("digest-mismatch")).await,
        Code::NotFound,
    );
    assert!(
        get(client, &upload_artifact_id("session", "digest-mismatch"))
            .await
            .is_none()
    );
    let nul = upload(client, "escaped-preview", &vec![0; 12000]).await;
    assert!(matches!(
        client
            .query_artifact(Query::ReadText {
                session_id: "session".into(),
                artifact_id: id(&nul)
            })
            .await
            .unwrap(),
        Queried::Text {
            preview: Err(ReadFailure::TooLarge),
            ..
        }
    ));
    assert!(matches!(
        client
            .query_artifact(Query::ReadBinary {
                session_id: "session".into(),
                artifact_id: id(&nul)
            })
            .await
            .unwrap(),
        Queried::Binary {
            preview: Err(BinaryReadFailure::UnsupportedMime),
            ..
        }
    ));
    let png = upload(client, "sniffed-preview", b"\x89PNG\r\n\x1a\n").await;
    assert!(
        matches!(client.query_artifact(Query::ReadBinary {session_id:"session".into(),artifact_id:id(&png)}).await.unwrap(),Queried::Binary {preview:Ok(p),..} if p.mime_type=="image/png")
    );
    let stale = page(client).await.0;
    for n in 0..129 {
        let key = format!("page-{n}");
        let mut input = begin(&key, b"");
        if let Ingest::Begin { name, .. } = &mut input {
            *name = "😀".repeat(60);
        }
        client.ingest_artifact(input).await.unwrap();
        client.ingest_artifact(commit(&key)).await.unwrap();
    }
    assert!(matches!(
        client
            .query_artifact(Query::ListContinue {
                session_id: "session".into(),
                revision: stale,
                cursor: "invalid".into()
            })
            .await
            .unwrap(),
        Queried::RevisionChanged { .. }
    ));
    let (revision, items) = page(client).await;
    assert_eq!(items.len(), 132);
    assert_eq!(
        items
            .iter()
            .map(|a| &a.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        132
    );
    rejected(
        client
            .query_artifact(Query::ListContinue {
                session_id: "session".into(),
                revision: revision.clone(),
                cursor: "01".into(),
            })
            .await,
        Code::InvalidRequest,
    );
    delete(client, "session", &id(&nul)).await.unwrap();
    assert!(get(client, &id(&nul)).await.is_none());
    assert_ne!(page(client).await.0, revision);

    let (disconnect, _disconnect_notices) = endpoint.open().await;
    disconnect
        .ingest_artifact(begin("disconnect", b"x"))
        .await
        .unwrap();
    disconnect.disconnect();
    tokio::time::timeout(Duration::from_secs(5), async {
        while client
            .request(Operation::HostStatus, json!({}))
            .await
            .unwrap()["connections"]
            != 2
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        sibling
            .ingest_artifact(begin("disconnect", b"x"))
            .await
            .unwrap(),
        Ingested::UploadOpened { .. }
    ));
    sibling.ingest_artifact(abort("disconnect")).await.unwrap();
    for n in 0..16 {
        client
            .ingest_artifact(begin(&format!("slot-{n}"), b""))
            .await
            .unwrap();
    }
    rejected(
        client.ingest_artifact(begin("overflow", b"")).await,
        Code::OperationConflict,
    );
    for n in 0..16 {
        client
            .ingest_artifact(abort(&format!("slot-{n}")))
            .await
            .unwrap();
    }
    client
        .ingest_artifact(begin("uncommitted", b"not published"))
        .await
        .unwrap();
    sibling.disconnect();
}
