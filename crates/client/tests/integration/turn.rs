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
use maka_protocol::turn::*;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn batch_and_query_bind_receipts_to_the_exact_turn_without_automatic_replay() {
    for query in [false, true] {
        for changed in [None, Some("sessionId"), Some("turnId")] {
            let (client, _notices, mut reader, mut writer) =
                pair_with(maka_client::Operations).await;
            let input = json!({"sessionId":"session","turnId":"turn","messages":[
                {"content":{"text":"first"}}, {"content":{"text":"second"},"inputSelections":{"example":["chosen"]}}
            ]});
            let task = tokio::spawn({
                let client = client.clone();
                let input = decode_turn_batch_start_input(&input).unwrap();
                async move {
                    if query {
                        client
                            .query_turn(TurnQueryInput {
                                session_id: input.session_id,
                                turn_id: input.turn_id,
                            })
                            .await
                            .map(|_| ())
                    } else {
                        client.start_turn_batch(input).await.map(|_| ())
                    }
                }
            });
            let frame = reader.read().await.unwrap().unwrap();
            assert_eq!(
                frame["operation"],
                if query {
                    "turn.query"
                } else {
                    "turn.batch.start"
                }
            );
            if !query {
                assert_eq!(frame["input"], input);
            }
            let mut turn =
                json!({"sessionId":"session","turnId":"turn","runId":"run","status":"running"});
            if let Some(field) = changed {
                turn[field] = json!("other");
            }
            let result = if query {
                turn
            } else {
                json!({"kind":"started","turn":turn,"preparation":[]})
            };
            writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
            let result = task.await.unwrap();
            if changed.is_some() {
                assert!(matches!(
                    result,
                    Err(RequestFailure::Unknown(ClientError::Protocol(_)))
                ));
                tokio::time::timeout(Duration::from_secs(1), client.closed())
                    .await
                    .unwrap();
            } else {
                result.unwrap();
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), reader.read())
                        .await
                        .is_err()
                );
            }
        }
    }
}
