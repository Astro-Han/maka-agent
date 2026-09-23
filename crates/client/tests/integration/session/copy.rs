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

use super::super::connection::pair_with;
use maka_client::{ClientError, RequestFailure};
use maka_protocol::session::{copy, sources};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn copy_sources_and_abandon_reject_cross_identity_and_changed_copy_intent() {
    for case in 0..9 {
        let input = copy::Input {
            source_session_id: "source".into(),
            target_session_id: "target".into(),
            expected_source_revision: 7,
            purpose: copy::Purpose::Revision {
                turn_id: "turn".into(),
            },
        };
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            let input = input.clone();
            async move {
                match case {
                    0..=4 => client.query_session_copy(input).await.map(|_| ()),
                    5 => client.copy_session(input).await.map(|_| ()),
                    6 => client
                        .abandon_session_revision(copy::AbandonInput {
                            target_session_id: "target".into(),
                        })
                        .await
                        .map(|_| ()),
                    _ => client
                        .session_turn_sources(sources::Input {
                            session_id: "source".into(),
                            turn_id: "turn".into(),
                        })
                        .await
                        .map(|_| ()),
                }
            }
        });
        let frame = reader.read().await.unwrap().unwrap();
        let result = match case {
            0..=4 => {
                let mut receipt = json!({"receipt":{"request":input,"state":"preparing"}});
                match case {
                    0 => receipt["receipt"]["request"]["sourceSessionId"] = json!("other"),
                    1 => receipt["receipt"]["request"]["targetSessionId"] = json!("other"),
                    2 => receipt["receipt"]["request"]["expectedSourceRevision"] = json!(8),
                    3 => receipt["receipt"]["request"]["purpose"]["turnId"] = json!("other"),
                    _ => {
                        receipt["receipt"]["request"]["purpose"] =
                            json!({"kind":"branch","turnId":"turn","sideConversation":true});
                        receipt["receipt"]["state"] = json!("committed");
                    }
                }
                receipt
            }
            5 => json!({"kind":"source_revision_conflict","expectedRevision":8,"actualRevision":9}),
            6 => json!({"kind":"abandoned","sessionId":"other"}),
            _ => {
                json!({"sessionId":if case == 7 {"source"} else {"other"},"turnId":if case == 7 {"other"} else {"turn"},"messages":[{"messageId":"message","content":{"text":"original input"}}]})
            }
        };
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        assert!(
            matches!(
                request.await.unwrap(),
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ),
            "case {case}"
        );
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
    }
}
