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
use maka_protocol::message::ExecutionResolution;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn stop_receipt_is_bound_to_the_exact_run_and_never_retried() {
    for changed in [None, Some("sessionId"), Some("turnId"), Some("runId")] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let task = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .stop_turn(maka_protocol::turn::TurnStopInput {
                        session_id: "session".into(),
                        turn_id: "turn".into(),
                        run_id: "run".into(),
                    })
                    .await
            }
        });
        let request = reader.read().await.unwrap().unwrap();
        assert_eq!(request["operation"], "turn.stop");
        assert_eq!(
            request["input"],
            json!({"sessionId":"session","turnId":"turn","runId":"run"})
        );
        let mut result = request["input"].clone();
        result["status"] = json!("running");
        if let Some(key) = changed {
            result[key] = json!("other");
        }
        writer.write(&json!({"requestId":request["requestId"],"operation":request["operation"],"ok":true,"result":result})).await.unwrap();
        let receipt = task.await.unwrap();
        if changed.is_none() {
            assert!(matches!(
                receipt.unwrap().state,
                maka_protocol::turn::TurnState::Running(_)
            ));
            assert!(
                tokio::time::timeout(Duration::from_millis(20), reader.read())
                    .await
                    .is_err()
            );
        } else {
            assert!(matches!(
                receipt,
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
            tokio::time::timeout(Duration::from_secs(1), client.closed())
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn execution_query_distinguishes_absence_and_receipts_and_rejects_unrequested_identities() {
    for (resolutions, valid) in [
        (json!([]), true),
        (json!([{"state":"not_admitted","messageId":"sent"}]), true),
        (json!([{"state":"not_admitted","messageId":"other"}]), false),
        (json!([{"state":"pending","messageId":"sent"}]), true),
        (json!([{"state":"cancelled","messageId":"sent"}]), true),
        (
            json!([{"state":"owned","messageId":"sent","turnId":"turn","runId":"run"}]),
            true,
        ),
        (json!([{"state":"pending","messageId":"other"}]), false),
        (
            json!([{"state":"pending","messageId":"sent"},{"state":"cancelled","messageId":"sent"}]),
            false,
        ),
    ] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let task = tokio::spawn({
            let client = client.clone();
            async move { client.message_execution("session", "sent").await }
        });
        let request = reader.read().await.unwrap().unwrap();
        assert_eq!(request["operation"], "turn.message.execution.query");
        assert_eq!(
            request["input"],
            json!({"sessionId":"session","messageIds":["sent"]})
        );
        writer
            .write(
                &json!({"requestId":request["requestId"],"operation":request["operation"],
            "ok":true,"result":{"resolutions":resolutions}}),
            )
            .await
            .unwrap();
        let result = task.await.unwrap();
        if valid {
            let expected: Option<ExecutionResolution> = resolutions
                .as_array()
                .unwrap()
                .first()
                .map(|value| serde_json::from_value(value.clone()).unwrap());
            assert_eq!(result.unwrap(), expected);
            assert!(
                tokio::time::timeout(Duration::from_millis(20), reader.read())
                    .await
                    .is_err(),
                "reading an outcome must not submit or retry anything"
            );
        } else {
            assert!(matches!(
                result,
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
            tokio::time::timeout(Duration::from_secs(1), client.closed())
                .await
                .unwrap();
        }
    }
}
