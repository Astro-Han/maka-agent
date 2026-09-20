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

use super::super::support::peer::Peer;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use maka_plugins::{
    composition::Scope,
    execution::{CommandError, Commands, CopyAttachment},
    fiber::Fiber,
};
use maka_runtime::{artifact::content_digest, attachment::StorageRef};
use maka_runtime_host::server::Host;
use serde_json::json;
use std::{sync::Arc, time::Duration};

pub(super) async fn verify(host: &Host, peer: &mut Peer, target: Arc<dyn Commands>, session: &str) {
    let bytes = vec![42; 128 * 1024];
    let begin = peer
        .rpc(
            "artifact.ingest",
            json!({
                "kind":"begin", "sessionId":"plugin-session", "uploadId":"transfer-source",
                "name":"source.bin", "mimeType":"application/octet-stream",
                "totalBytes":bytes.len(), "contentSha256":content_digest(&bytes)
            }),
        )
        .await;
    assert_eq!(begin["ok"], true, "{begin}");
    if begin["result"]["kind"] != "committed" {
        for (index, chunk) in bytes.chunks(32 * 1024).enumerate() {
            let result = peer
                .rpc(
                    "artifact.ingest",
                    json!({
                        "kind":"chunk", "sessionId":"plugin-session", "uploadId":"transfer-source",
                        "offset":index * 32 * 1024, "chunkBase64":STANDARD.encode(chunk)
                    }),
                )
                .await;
            assert_eq!(result["ok"], true, "{result}");
        }
    }
    let uploaded = peer
        .rpc(
            "artifact.ingest",
            json!({
                "kind":"commit", "sessionId":"plugin-session", "uploadId":"transfer-source"
            }),
        )
        .await;
    assert_eq!(uploaded["ok"], true, "{uploaded}");
    let source = Fiber::new("source-reader", "source-reader", Scope::Profile).unwrap();
    source.begin_loading().unwrap();
    source.ready().unwrap();
    source.publish().unwrap();
    let commands = host
        .authorize_plugin_execution(source.context(), &["plugin-session".into()])
        .await
        .unwrap();
    let request = CopyAttachment {
        target_session_id: session.into(),
        attachment: serde_json::from_value(uploaded["result"]["attachment"].clone()).unwrap(),
    };
    assert!(matches!(
        commands
            .copy_attachment(commands.clone(), request.clone())
            .await,
        Err(CommandError::Denied)
    ));
    let copied = target
        .copy_attachment(commands.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        target
            .copy_attachment(commands.clone(), request.clone())
            .await
            .unwrap(),
        copied
    );
    let StorageRef::SessionFile {
        session_id,
        relative_path,
    } = &copied.storage_ref
    else {
        panic!("immutable destination")
    };
    assert_eq!(session_id, session);
    for offset in (0..bytes.len()).step_by(32 * 1024) {
        let result = peer.rpc("artifact.query", json!({
            "kind":"read_chunk", "sessionId":session, "artifactId":relative_path, "offset":offset,
        })).await;
        assert_eq!(result["ok"], true, "{result}");
        assert_eq!(
            STANDARD
                .decode(result["result"]["chunkBase64"].as_str().unwrap())
                .unwrap(),
            bytes[offset..offset + 32 * 1024]
        );
    }
    let mut forged = request.clone();
    forged.attachment.name = "forged.bin".into();
    assert!(
        target
            .copy_attachment(commands.clone(), forged)
            .await
            .is_err()
    );
    source
        .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    assert!(matches!(
        target.copy_attachment(commands, request).await,
        Err(CommandError::Revoked)
    ));
}
