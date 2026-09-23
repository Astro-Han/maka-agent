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
use maka_protocol::interaction::{Decision, InteractionAnswer, decode_snapshot};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn interaction_receipts_bind_original_request_and_queries_never_reanswer() {
    let pending = json!({
        "schemaVersion":1,"interactionId":"approval","sessionId":"session","turnId":"turn","runId":"run",
        "revision":1,"status":"pending","outcome":null,
        "request":{"kind":"client_capability","toolUseId":"call",
            "target":{"providerId":"provider","contractId":"contract","serverId":"server","toolName":"browser",
                "capability":"browser","scope":{"kind":"browser_origin","origin":"https://example.com"}}}
    });
    for change in [
        "valid",
        "interactionId",
        "sessionId",
        "turnId",
        "runId",
        "request",
        "decision",
    ] {
        let expected = decode_snapshot(&pending).unwrap();
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let job = tokio::spawn({
            let client = client.clone();
            let expected = expected.clone();
            async move {
                client
                    .answer_interaction(
                        &expected,
                        InteractionAnswer::ClientCapability {
                            decision: Decision::Deny,
                        },
                    )
                    .await
            }
        });
        let request = reader.read().await.unwrap().unwrap();
        assert_eq!(request["operation"], "interaction.answer");
        assert_eq!(
            request["input"],
            json!({"sessionId":"session","interactionId":"approval","answer":{"kind":"client_capability","decision":"deny"}})
        );
        let mut receipt = pending.clone();
        receipt["revision"] = json!(2);
        receipt["status"] = json!("answered");
        receipt["outcome"] =
            json!({"kind":"client_capability_decision","decision":"deny","committedAt":1});
        match change {
            "valid" => {}
            "request" => receipt["request"]["target"]["providerId"] = json!("different"),
            "decision" => receipt["outcome"]["decision"] = json!("allow"),
            field => receipt[field] = json!("different"),
        }
        writer.write(&json!({"requestId":request["requestId"],"operation":request["operation"],"ok":true,"result":receipt})).await.unwrap();
        let result = job.await.unwrap();
        if change != "valid" {
            assert!(matches!(
                result,
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
            tokio::time::timeout(Duration::from_secs(1), client.closed())
                .await
                .unwrap();
            continue;
        }
        assert_eq!(result.unwrap(), decode_snapshot(&receipt).unwrap());
        let query = tokio::spawn({
            let client = client.clone();
            async move { client.interaction(&expected).await }
        });
        let request = reader.read().await.unwrap().unwrap();
        assert_eq!(request["operation"], "interaction.query");
        assert_eq!(
            request["input"],
            json!({"sessionId":"session","interactionId":"approval"})
        );
        receipt["status"] = json!("closed");
        receipt["outcome"] = json!({"kind":"closure","reason":"host_restarted","committedAt":2});
        writer.write(&json!({"requestId":request["requestId"],"operation":request["operation"],"ok":true,"result":receipt})).await.unwrap();
        let closed = query.await.unwrap().unwrap();
        assert_eq!(closed, decode_snapshot(&receipt).unwrap());
        assert!(matches!(
            client
                .answer_interaction(
                    &closed,
                    InteractionAnswer::ClientCapability {
                        decision: Decision::Allow
                    }
                )
                .await,
            Err(RequestFailure::NotDispatched(_))
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), reader.read())
                .await
                .is_err()
        );
    }
}
