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
use maka_client::{ClientError, Notification};
use maka_protocol::{Operation, subscription::*};
use serde_json::{Value, json};
use std::time::Duration;

fn input() -> SubscriptionOpenInput {
    SubscriptionOpenInput {
        session_id: "s".into(),
        transcript: TranscriptPolicy::None,
    }
}
fn snapshot() -> Value {
    json!({
        "schemaVersion":5,
        "session":{"sessionId":"s","metadataRevision":1,"status":"active","createdAt":0,"isArchived":false},
        "projectionRevision":1,"rootTurn":null,"goal":null,
        "queue":{"hostEpoch":"epoch-test","queueRevision":0,"steering":[],"followup":[]},
        "interactions":{"pending":[]}
    })
}
fn open() -> Value {
    json!({"hostEpoch":"epoch-test","subscriptionId":"sub","nextSequence":1,
        "snapshot":snapshot(),"activeAssistantStreams":[],"transcript":null})
}
fn reply(request: &Value, result: Value) -> Value {
    json!({"requestId":request["requestId"],"operation":request["operation"],"ok":true,"result":result})
}
fn advanced(sequence: u64, through: u64) -> Value {
    json!({"kind":"subscription.transcript_advanced","hostEpoch":"epoch-test",
        "subscriptionId":"sub","sequence":sequence,"sessionId":"s","throughSequence":through})
}

#[tokio::test]
async fn snapshot_then_ready_allows_early_frames_and_close_preserves_connection() {
    let (client, mut notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
    let task = tokio::spawn({
        let client = client.clone();
        async move { client.open_subscription(input()).await }
    });
    let request = reader.read().await.unwrap().unwrap();
    writer.write(&reply(&request, open())).await.unwrap();
    let opened = task.await.unwrap().unwrap();
    assert_eq!(opened.snapshot.session.session_id, "s");
    assert!(notices.try_recv().is_err());

    let ready = tokio::spawn({
        let client = client.clone();
        async move { client.ready_subscription("sub").await }
    });
    let request = reader.read().await.unwrap().unwrap();
    // The Host may enqueue catch-up before its ready acknowledgement.
    writer.write(&advanced(1, 4)).await.unwrap();
    assert!(matches!(
        notices.recv().await,
        Some(Notification::Observation(_))
    ));
    writer
        .write(&reply(&request, json!({"subscriptionId":"sub"})))
        .await
        .unwrap();
    ready.await.unwrap().unwrap();

    let mut projection = snapshot();
    projection["projectionRevision"] = json!(2);
    let frames = [
        json!({"kind":"subscription.session_projection","hostEpoch":"epoch-test","subscriptionId":"sub","sequence":2,"snapshot":projection}),
        json!({"kind":"subscription.session_delta","hostEpoch":"epoch-test","subscriptionId":"sub","sequence":3,"sessionId":"s",
            "delta":{"kind":"text","turnId":"t","runId":"r","messageId":"m","startOffset":0,"text":"你好 🦀"}}),
        // PTY sequence is independent and must not consume main sequence 4.
        json!({"kind":"subscription.runtime_resource_pty_data","hostEpoch":"epoch-test","subscriptionId":"sub","sessionId":"s","ref":"pty:one","ptySequence":9,"data":"output"}),
        json!({"kind":"subscription.agent_graph_changed","hostEpoch":"epoch-test","subscriptionId":"sub","sequence":4,"rootSessionId":"s","graphId":"g","reason":"observation"}),
    ];
    for frame in frames {
        writer.write(&frame).await.unwrap();
        assert!(matches!(
            notices.recv().await,
            Some(Notification::Observation(_))
        ));
    }
    // Exhaust ordinary admission. Reserved control slots must still close the
    // observation without waiting for unrelated domain replies.
    let mut domain_jobs = tokio::task::JoinSet::new();
    for _ in 0..60 {
        let client = client.clone();
        domain_jobs.spawn(async move { client.request(Operation::HostWake, json!({})).await });
    }
    let mut domain_requests = Vec::new();
    for _ in 0..60 {
        domain_requests.push(
            tokio::time::timeout(Duration::from_secs(2), reader.read())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        );
    }
    let close = tokio::spawn({
        let client = client.clone();
        async move { client.close_subscription("sub").await }
    });
    let request = tokio::time::timeout(Duration::from_secs(2), reader.read())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(request["operation"], "subscription.close");
    writer.write(&advanced(5, 5)).await.unwrap(); // Already queued before close.
    assert!(matches!(
        notices.recv().await,
        Some(Notification::Observation(_))
    ));
    writer
        .write(&reply(&request, json!({"subscriptionId":"sub"})))
        .await
        .unwrap();
    close.await.unwrap().unwrap();
    for request in domain_requests {
        writer.write(&reply(&request, json!({}))).await.unwrap();
    }
    while let Some(result) = domain_jobs.join_next().await {
        result.unwrap().unwrap();
    }
    writer
        .write(&json!({"kind":"configuration.changed","revision":1}))
        .await
        .unwrap();
    assert!(matches!(
        notices.recv().await,
        Some(Notification::Catalog(_))
    ));
}

#[tokio::test]
async fn subscription_rejects_bad_identity_order_revision_watermark_and_unready_frames() {
    let mut projection = snapshot();
    projection["projectionRevision"] = json!(1); // Not newer than the snapshot.
    let projection = json!({"kind":"subscription.session_projection","hostEpoch":"epoch-test","subscriptionId":"sub","sequence":2,"snapshot":projection});
    let cases = [
        ("sequence", json!(3)),
        ("hostEpoch", json!("other")),
        ("subscriptionId", json!("other")),
        ("sessionId", json!("other")),
        ("throughSequence", json!(4)),
        ("projection", projection),
        ("before_ready", Value::Null),
        ("after_closed", Value::Null),
    ];
    for (field, value) in cases {
        let (client, mut notices, mut reader, mut writer) =
            pair_with(maka_client::Operations).await;
        let task = tokio::spawn({
            let client = client.clone();
            async move { client.open_subscription(input()).await }
        });
        let request = reader.read().await.unwrap().unwrap();
        writer.write(&reply(&request, open())).await.unwrap();
        task.await.unwrap().unwrap();
        let mut bad = advanced(2, 5);
        if field == "before_ready" {
            bad = advanced(1, 4);
        } else {
            let ready = tokio::spawn({
                let client = client.clone();
                async move { client.ready_subscription("sub").await }
            });
            let request = reader.read().await.unwrap().unwrap();
            writer
                .write(&reply(&request, json!({"subscriptionId":"sub"})))
                .await
                .unwrap();
            ready.await.unwrap().unwrap();
            let first = if field == "after_closed" {
                json!({"kind":"subscription.closed","hostEpoch":"epoch-test","subscriptionId":"sub","sequence":1,"reason":"slow_consumer"})
            } else {
                advanced(1, 4)
            };
            writer.write(&first).await.unwrap();
            notices.recv().await.unwrap();
            if field == "projection" {
                bad = value;
            } else if field != "after_closed" {
                bad[field] = value;
            }
        }
        writer.write(&bad).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(2), client.closed())
            .await
            .unwrap();
        assert!(
            matches!(error, ClientError::Protocol(_)),
            "{field}: {error}"
        );
        assert!(
            notices.recv().await.is_none(),
            "invalid frame escaped: {field}"
        );
    }
}

#[tokio::test]
async fn open_correlation_and_abandoned_open_cannot_leave_an_unowned_subscription() {
    for case in ["session", "epoch", "policy", "abandoned"] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let task = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .request_with_timeout(
                        Operation::SubscriptionOpen,
                        serde_json::to_value(input()).unwrap(),
                        Duration::from_millis(100),
                    )
                    .await
            }
        });
        let request = reader.read().await.unwrap().unwrap();
        let mut result = open();
        match case {
            "session" => result["snapshot"]["session"]["sessionId"] = json!("other"),
            "epoch" => {
                result["hostEpoch"] = json!("other");
                result["snapshot"]["queue"]["hostEpoch"] = json!("other");
            }
            "policy" => {
                result["transcript"] = json!({"durable":{"kind":"page","sessionId":"s","direction":"older","throughSequence":null,"rawBytes":0,"fragments":[],"endsAtTurnBoundary":true,"nextCursor":null}})
            }
            "abandoned" => {
                task.abort();
                let _ = task.await;
                writer.write(&reply(&request, result)).await.unwrap();
                assert!(matches!(
                    tokio::time::timeout(Duration::from_secs(2), client.closed())
                        .await
                        .unwrap(),
                    ClientError::Closed(_)
                ));
                continue;
            }
            _ => unreachable!(),
        }
        writer.write(&reply(&request, result)).await.unwrap();
        assert!(task.await.unwrap().is_err());
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), client.closed())
                .await
                .unwrap(),
            ClientError::Protocol(_)
        ));
    }
}

#[tokio::test]
async fn subscription_acknowledgements_and_pages_are_bound_to_the_request() {
    use maka_protocol::transcript::{SessionTranscriptPageDirection, SessionTranscriptPageInput};
    for case in [
        "ack",
        "sessionId",
        "direction",
        "throughSequence",
        "cursor",
        "search_session",
        "search_fence",
        "search_cursor",
        "search_limit",
    ] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let task = tokio::spawn({
            let client = client.clone();
            async move { client.open_subscription(input()).await }
        });
        let request = reader.read().await.unwrap().unwrap();
        writer.write(&reply(&request, open())).await.unwrap();
        task.await.unwrap().unwrap();
        let task = tokio::spawn({
            let client = client.clone();
            async move {
                if case == "ack" {
                    client.ready_subscription("sub").await
                } else if case.starts_with("search_") {
                    client
                        .transcript_search(maka_protocol::transcript::TranscriptSearchInput {
                            subscription_id: "sub".into(),
                            through_sequence: Some(5),
                            query: "needle".into(),
                            include_internal: false,
                            cursor: Some("cursor".into()),
                            max_matches: 1,
                        })
                        .await
                        .map(|_| ())
                } else {
                    client
                        .transcript_page(SessionTranscriptPageInput {
                            subscription_id: "sub".into(),
                            direction: SessionTranscriptPageDirection::Older,
                            through_sequence: Some(5),
                            cursor: (case == "cursor").then(|| "cursor".into()),
                            anchor_sequence: None,
                            max_bytes: 1024,
                        })
                        .await
                        .map(|_| ())
                }
            }
        });
        let request = reader.read().await.unwrap().unwrap();
        let mut result = json!({"kind":"page","sessionId":"s","direction":"older","throughSequence":5,
            "rawBytes":0,"fragments":[],"endsAtTurnBoundary":true,"nextCursor":null});
        if case.starts_with("search_") {
            result = json!({"sessionId":"s", "throughSequence":5,"matches":[],"nextCursor":null});
        }
        match case {
            "ack" => result = json!({"subscriptionId":"other"}),
            "sessionId" => result["sessionId"] = json!("other"),
            "direction" => result["direction"] = json!("newer"),
            "throughSequence" => result["throughSequence"] = json!(6),
            "cursor" => result["nextCursor"] = json!("cursor"),
            "search_session" => result["sessionId"] = json!("other"),
            "search_fence" => result["throughSequence"] = json!(6),
            "search_cursor" => result["nextCursor"] = json!("cursor"),
            "search_limit" => {
                result["matches"] =
                    json!([{"sequence":1,"preview":"needle"},{"sequence":2,"preview":"needle"}])
            }
            _ => unreachable!(),
        }
        writer.write(&reply(&request, result)).await.unwrap();
        assert!(task.await.unwrap().is_err(), "{case}");
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), client.closed())
                .await
                .unwrap(),
            ClientError::Protocol(_)
        ));
    }
}
