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

use super::connection::pair_with;
use maka_client::{ClientError, RequestFailure};
use maka_protocol::{Operation, artifact::*};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn artifact_responses_cannot_cross_upload_session_or_pagination_boundaries() {
    let digest = format!("sha256:{}", "0".repeat(64));
    let begin = json!({"kind":"begin","sessionId":"s","uploadId":"u","name":"file.txt",
        "mimeType":"text/plain","totalBytes":2,"contentSha256":digest});
    let receipt = json!({"kind":"committed","uploadId":"u","attachment":{
        "kind":"other","name":"file.txt","mimeType":"text/plain","bytes":2,
        "ref":{"kind":"session_file","sessionId":"s","relativePath":upload_artifact_id("s","u")}}});
    let record = json!({"id":"a","sessionId":"s","turnId":"u","createdAt":1,"name":"file.txt",
        "kind":"file","sizeBytes":2,"mimeType":"text/plain","source":"user_upload"});
    let mut cases = vec![
        (
            Operation::ArtifactIngest,
            begin.clone(),
            json!({"kind":"upload_opened","uploadId":"other","nextOffset":0}),
        ),
        (
            Operation::ArtifactIngest,
            begin.clone(),
            json!({"kind":"upload_opened","uploadId":"u","nextOffset":3}),
        ),
        (
            Operation::ArtifactIngest,
            begin.clone(),
            json!({"kind":"upload_aborted","uploadId":"u"}),
        ),
        (
            Operation::ArtifactIngest,
            json!({"kind":"chunk","sessionId":"s","uploadId":"u","offset":2,"chunkBase64":"eA=="}),
            json!({"kind":"chunk_accepted","uploadId":"u","nextOffset":2}),
        ),
        (
            Operation::ArtifactIngest,
            json!({"kind":"abort","sessionId":"s","uploadId":"u"}),
            receipt.clone(),
        ),
        (
            Operation::ArtifactQuery,
            json!({"kind":"get","sessionId":"s","artifactId":"a"}),
            json!({"kind":"artifact","sessionId":"other","revision":digest,"artifact":null}),
        ),
        (
            Operation::ArtifactQuery,
            json!({"kind":"read_text","sessionId":"s","artifactId":"a"}),
            json!({"kind":"text","sessionId":"s","artifactId":"other","preview":{"ok":true,"text":"x"}}),
        ),
        (
            Operation::ArtifactQuery,
            json!({"kind":"read_chunk","sessionId":"s","artifactId":"a","offset":1}),
            json!({"kind":"chunk","sessionId":"s","artifactId":"a","offset":0,"totalBytes":1,"chunkBase64":"eA==","nextOffset":null}),
        ),
        (
            Operation::ArtifactQuery,
            json!({"kind":"list_continue","sessionId":"s","revision":digest,"cursor":"1"}),
            json!({"kind":"page","sessionId":"s","revision":digest,"artifacts":[record.clone()],"nextCursor":"1"}),
        ),
        (
            Operation::ArtifactQuery,
            json!({"kind":"list_start","sessionId":"s"}),
            json!({"kind":"page","sessionId":"s","revision":digest,"artifacts":[record.clone(),record.clone()],"nextCursor":null}),
        ),
        (
            Operation::ArtifactQuery,
            json!({"kind":"list_continue","sessionId":"s","revision":digest,"cursor":"1"}),
            json!({"kind":"revision_changed","expected":digest,"actual":digest}),
        ),
    ];
    for (pointer, value) in [
        ("/attachment/ref/sessionId", json!("other")),
        (
            "/attachment/ref/relativePath",
            json!(upload_artifact_id("s", "another-upload")),
        ),
        ("/attachment/name", json!("other.txt")),
        ("/attachment/mimeType", json!("text/markdown")),
        ("/attachment/bytes", json!(3)),
        ("/attachment/kind", json!("image")),
    ] {
        let mut changed = receipt.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        cases.push((Operation::ArtifactIngest, begin.clone(), changed));
    }
    for (pointer, value) in [("/id", json!("other")), ("/sessionId", json!("other"))] {
        let mut changed = record.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        cases.push((
            Operation::ArtifactQuery,
            json!({"kind":"get","sessionId":"s","artifactId":"a"}),
            json!({"kind":"artifact","sessionId":"s","revision":digest,"artifact":changed}),
        ));
    }
    for (index, (operation, input, result)) in cases.into_iter().enumerate() {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move {
                match operation {
                    Operation::ArtifactIngest => client
                        .ingest_artifact(serde_json::from_value(input).unwrap())
                        .await
                        .map(|_| ()),
                    Operation::ArtifactQuery => client
                        .query_artifact(serde_json::from_value(input).unwrap())
                        .await
                        .map(|_| ()),
                    _ => unreachable!(),
                }
            }
        });
        let frame = tokio::time::timeout(Duration::from_secs(2), reader.read())
            .await
            .expect("Artifact request must reach the transport")
            .unwrap()
            .unwrap();
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        assert!(
            matches!(
                request.await.unwrap(),
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ),
            "case {index}"
        );
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
    }
}
