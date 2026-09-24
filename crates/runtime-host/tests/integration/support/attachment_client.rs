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
use maka_event_log::root::RootOwner;
use maka_protocol::{
    Operation, OperationErrorCode,
    artifact::{ArtifactIngestInput as Ingest, ArtifactIngestResult},
    subscription::{SubscriptionOpenInput, TranscriptPolicy},
    transcript::{SessionTranscriptPageDirection, SessionTranscriptPageInput},
    turn::AttachmentRef,
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub struct NativeHost {
    pub client: Client,
    pub notices: tokio::sync::mpsc::Receiver<maka_client::Notification>,
    cancel: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), maka_runtime_host::server::HostError>>,
}
impl NativeHost {
    pub async fn open(owner: RootOwner, socket: &Path) -> Self {
        let root = owner.root_id().to_owned();
        let host = Host::open(owner).await.unwrap();
        let (peer, hello) =
            super::peer::Peer::handshake(host.clone(), "attachments-bootstrap").await;
        peer.close().await;
        let cancel = CancellationToken::new();
        let server = tokio::spawn(
            LocalListener::bind(socket)
                .unwrap()
                .serve(host, cancel.clone()),
        );
        let (client, notices) = Client::connect(
            maka_client::local::open_stream(socket).await.unwrap(),
            &root,
            hello["hostEpoch"].as_str().unwrap(),
            maka_client::Operations,
        )
        .await
        .unwrap();
        Self {
            client,
            notices,
            cancel,
            server,
        }
    }
    pub async fn close(mut self) {
        self.client.disconnect();
        self.cancel.cancel();
        tokio::time::timeout(Duration::from_secs(10), &mut self.server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
impl Drop for NativeHost {
    fn drop(&mut self) {
        self.client.disconnect();
        self.cancel.cancel();
    }
}

pub async fn configure(client: &Client, url: &str, models: Value) -> Value {
    let created = super::model_connection::create(
        client,
        "openai",
        "attachments",
        url,
        "attachment-fixture",
        models,
    )
    .await;
    let basis = &created["connection"];
    basis["connectionId"].clone()
}
pub async fn session(client: &Client, connection: &Value, workspace: &Path, id: &str, model: &str) {
    client.create_session(maka_protocol::session::decode_session_create_input(&json!({
        "sessionId":id,"workspace":{"kind":"host_path","path":workspace},
        "modelTarget":{"kind":"explicit","connectionId":connection,"connectionSlug":"attachments","model":model},
        "sandboxMode":"read-only","mode":"bot"
    })).unwrap()).await.unwrap();
}
pub async fn upload(
    client: &Client,
    session: &str,
    id: &str,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> AttachmentRef {
    client
        .ingest_artifact(Ingest::Begin {
            session_id: session.into(),
            upload_id: id.into(),
            name: name.into(),
            mime_type: mime.into(),
            total_bytes: bytes.len() as u64,
            content_sha256: maka_runtime::artifact::content_digest(bytes),
        })
        .await
        .unwrap();
    for (index, part) in bytes.chunks(48 * 1024).enumerate() {
        client
            .ingest_artifact(Ingest::Chunk {
                session_id: session.into(),
                upload_id: id.into(),
                offset: (index * 48 * 1024) as u64,
                chunk_base64: STANDARD.encode(part),
            })
            .await
            .unwrap();
    }
    let ArtifactIngestResult::Committed { attachment, .. } = client
        .ingest_artifact(Ingest::Commit {
            session_id: session.into(),
            upload_id: id.into(),
        })
        .await
        .unwrap()
    else {
        panic!("expected committed upload");
    };
    attachment
}
pub async fn completed(client: &Client, turn: &Value) -> Value {
    client
        .request(Operation::TurnStart, turn.clone())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let snapshot = client
                .request(
                    Operation::TurnQuery,
                    json!({
                        "sessionId":turn["sessionId"],"turnId":turn["turnId"]
                    }),
                )
                .await
                .unwrap();
            match snapshot["status"].as_str().unwrap() {
                "completed" => break snapshot,
                "failed" | "cancelled" => panic!("unexpected terminal turn: {snapshot}"),
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .unwrap()
}
pub fn rejected<T: std::fmt::Debug>(result: Result<T, RequestFailure>, code: OperationErrorCode) {
    assert!(
        matches!(&result, Err(RequestFailure::Rejected(ClientError::Rejected(error))) if error.code==code),
        "{result:?}"
    );
}
pub async fn rows(client: &Client, session: &str) -> Vec<Value> {
    let open = client
        .open_subscription(SubscriptionOpenInput {
            session_id: session.into(),
            transcript: TranscriptPolicy::Tail { max_bytes: 2 },
        })
        .await
        .unwrap();
    let mut page = open.transcript.unwrap().durable;
    assert!(page.next_cursor.is_some(), "exercise fragmented bootstrap");
    let mut rows = Vec::new();
    loop {
        let batch = client
            .complete_transcript_page(&open.subscription_id, page)
            .await
            .unwrap();
        rows.extend(batch.rows.into_iter().map(|row| (row.sequence, row.value)));
        let Some(cursor) = batch.next_cursor else {
            break;
        };
        page = client
            .transcript_page(SessionTranscriptPageInput {
                subscription_id: open.subscription_id.clone(),
                direction: SessionTranscriptPageDirection::Older,
                through_sequence: batch.through_sequence,
                cursor: Some(cursor),
                anchor_sequence: None,
                max_bytes: 48 * 1024,
            })
            .await
            .unwrap();
    }
    client
        .close_subscription(&open.subscription_id)
        .await
        .unwrap();
    rows.sort_by_key(|(sequence, _)| *sequence);
    assert!(rows.windows(2).all(|pair| pair[0].0 < pair[1].0));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// A real HTTP/SSE provider; assertions run against serialized model requests.
pub struct Model {
    pub url: String,
    server: tokio::task::JoinHandle<()>,
}
impl Model {
    pub async fn start(
        count: usize,
        response: impl Fn(usize, &Value) -> Value + Send + Sync + 'static,
    ) -> Self {
        use http_body_util::{BodyExt, Full, Limited};
        use hyper::{
            Request, Response,
            body::{Bytes, Incoming},
            server::conn::http1,
            service::service_fn,
        };
        use hyper_util::rt::TokioIo;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let response = Arc::new(response);
        let server = tokio::spawn(async move {
            for index in 0..count {
                let (stream, _) = listener.accept().await.unwrap();
                let response = response.clone();
                http1::Builder::new().serve_connection(TokioIo::new(stream), service_fn(move |request: Request<Incoming>| {
                    let response = response.clone();
                    async move {
                        assert_eq!(request.uri().path(), "/v1/chat/completions");
                        assert_eq!(request.headers()["authorization"], "Bearer attachment-fixture");
                        let bytes = Limited::new(request.into_body(),128*1024).collect().await.unwrap().to_bytes();
                        let input: Value = serde_json::from_slice(&bytes).unwrap();
                        assert_eq!(input["stream"],true);
                        let delta = response(index,&input);
                        let finish = if delta.get("tool_calls").is_some() { "tool_calls" } else { "stop" };
                        let event = json!({"id":"attachment-fixture","object":"chat.completion.chunk","model":input["model"],
                            "choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
                        Ok::<_,std::convert::Infallible>(Response::builder().status(200)
                            .header("content-type","text/event-stream").header("connection","close")
                            .body(Full::new(Bytes::from(format!("data: {event}\n\ndata: [DONE]\n\n")))).unwrap())
                    }
                })).await.unwrap();
            }
        });
        Self { url, server }
    }
    pub async fn finish(mut self) {
        tokio::time::timeout(Duration::from_secs(10), &mut self.server)
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for Model {
    fn drop(&mut self) {
        self.server.abort();
    }
}
pub fn read(id: &str, artifact: &str) -> Value {
    json!({"tool_calls":[{"index":0,"id":id,"type":"function","function":{
        "name":"Read","arguments":json!({"path":format!("maka://runtime/attachments/{artifact}")}).to_string()
    }}]})
}
