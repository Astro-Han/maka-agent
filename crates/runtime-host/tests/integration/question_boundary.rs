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

#![cfg(unix)]
use maka_event_log::root::{ROOT_DATABASE, RootOwner};
use maka_runtime_host::server::{Host, local::LocalListener};
use maka_transport::MessageReader;
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::support::execution_fixture as support;
use crate::support::question_model as model;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn question_commit_failures_never_publish_an_unjournaled_question_or_deliver_an_answer() {
    for cut in ["dispatch", "request", "answer"] {
        let (directory, ns, root, provider, host) = support::fixture().await;
        let mut database = SqliteConnection::connect_with(
            &SqliteConnectOptions::new().filename(root.join(ROOT_DATABASE)),
        )
        .await
        .unwrap();
        let trigger = match cut {
            "dispatch" => {
                "CREATE TRIGGER fail_question BEFORE INSERT ON event_log
                WHEN NEW.kind = 'tool_dispatched'
                BEGIN SELECT RAISE(ABORT, 'dispatch failure'); END;"
            }
            "request" => {
                "CREATE TRIGGER fail_question BEFORE INSERT ON interaction_requests
                BEGIN SELECT RAISE(ABORT, 'request failure'); END;"
            }
            "answer" => {
                "CREATE TRIGGER fail_question BEFORE INSERT ON interaction_outcomes
                WHEN json_extract(NEW.outcome_json, '$.kind') = 'question_answer'
                BEGIN SELECT RAISE(ABORT, 'answer failure'); END;"
            }
            _ => unreachable!(),
        };
        sqlx::raw_sql(trigger).execute(&mut database).await.unwrap();
        let model = model::ask_question(&provider);
        let socket_path = directory.path().join("h.sock");
        let server = tokio::spawn(
            LocalListener::bind(&socket_path)
                .unwrap()
                .serve(host, CancellationToken::new()),
        );
        let socket = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let (mut reader, mut writer) =
            maka_transport::ndjson::split(socket, CancellationToken::new());
        writer
            .write(
                &json!({"kind":"hello","clientInstanceId":"question-boundary",
            "protocolMin":0,"protocolMax":0,
            "compatibilityEpoch":maka_protocol::COMPATIBILITY_EPOCH,"compositionId":"maka.interactive"}),
            )
            .await
            .unwrap();
        assert_eq!(reader.read().await.unwrap().unwrap()["state"], "ready");
        writer
            .write(&request(
                "turn.start",
                json!({"sessionId":"session","turnId":"turn",
            "content":{"text":"ask once"},"maxSteps":2}),
            ))
            .await
            .unwrap();
        assert_eq!(response(&mut reader).await["result"]["kind"], "started");
        if cut == "answer" {
            let id = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let id: Option<String> =
                        sqlx::query_scalar("SELECT request_id FROM interaction_requests")
                            .fetch_optional(&mut database)
                            .await
                            .unwrap();
                    if let Some(id) = id {
                        break id;
                    }
                    assert!(
                        !server.is_finished(),
                        "Host ended before Question publication"
                    );
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(count(&mut database, "tool_dispatched").await, 1);
            assert_eq!(count(&mut database, "tool_settled").await, 0);
            writer
                .write(&request(
                    maka_protocol::Operation::SessionCatalogQuery.as_str(),
                    json!({"kind":"get","sessionId":"session"}),
                ))
                .await
                .unwrap();
            let revision = response(&mut reader).await["result"]["session"]["revision"].clone();
            writer
                .write(&request(
                    maka_protocol::Operation::SessionConfigurationUpdate.as_str(),
                    json!({"sessionId":"session","expectedRevision":revision,
                    "patch":{"sandboxMode":"danger-full-access"}}),
                ))
                .await
                .unwrap();
            let pending = response(&mut reader).await;
            assert_eq!(pending["error"]["code"], "session_busy", "{pending}");
            assert_eq!(
                pending["error"]["message"],
                "Session has a pending Interaction"
            );
            writer
                .write(&request(
                    "interaction.answer",
                    json!({"sessionId":"session",
                "interactionId":id,"answer":{"kind":"question","answers":["A"]}}),
                ))
                .await
                .unwrap();
            assert_eq!(
                response(&mut reader).await["error"]["code"],
                "internal_failure"
            );
        }
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), model)
            .await
            .unwrap()
            .unwrap();
        drop(reader);
        drop(writer);
        assert_eq!(
            count(&mut database, "model_requested").await,
            1,
            "{cut}: no answer may reach a second provider call"
        );
        assert_eq!(
            count(&mut database, "tool_dispatched").await,
            i64::from(cut != "dispatch")
        );
        let requests: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM interaction_requests")
            .fetch_one(&mut database)
            .await
            .unwrap();
        assert_eq!(
            requests,
            i64::from(cut == "answer"),
            "{cut}: T1 precedes publication"
        );
        let answered: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM interaction_outcomes WHERE json_extract(outcome_json, '$.kind') = 'question_answer'",
        ).fetch_one(&mut database).await.unwrap();
        assert_eq!(answered, 0, "{cut}: never invent an accepted answer");
        let successes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM runtime_events WHERE kind = 'tool_settled' AND
             json_extract(event_json, '$.fact.outcome.kind') = 'succeeded'",
        )
        .fetch_one(&mut database)
        .await
        .unwrap();
        assert_eq!(successes, 0);
        if cut != "answer" {
            assert_eq!(
                count(&mut database, "tool_settled").await,
                0,
                "{cut}: persistence uncertainty must not become a known tool result"
            );
        }
        let before = canonical(&mut database).await;
        sqlx::raw_sql("DROP TRIGGER fail_question")
            .execute(&mut database)
            .await
            .unwrap();
        database.close().await.unwrap();

        // Release the complete root owner before reopening. Startup may seal an
        // interrupted invocation, but cannot replay the Question or model call.
        let reopened = Host::open(RootOwner::open(&root, &ns).unwrap())
            .await
            .unwrap();
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        LocalListener::bind(&socket_path)
            .unwrap()
            .serve(reopened, shutdown)
            .await
            .unwrap();
        let mut database = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(root.join(ROOT_DATABASE))
                .read_only(true),
        )
        .await
        .unwrap();
        let after = canonical(&mut database).await;
        assert_eq!(
            after.0[..before.0.len()],
            before.0,
            "{cut}: canonical prefix changed"
        );
        assert_eq!(after.1, before.1, "{cut}: Question was republished");
        assert_eq!(count(&mut database, "model_requested").await, 1);
        assert_eq!(
            count(&mut database, "tool_dispatched").await,
            i64::from(cut != "dispatch")
        );
        assert_eq!(
            provider.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        database.close().await.unwrap();
    }
}

fn request(operation: &str, input: Value) -> Value {
    json!({"requestId":operation,"operation":operation,"input":input})
}
async fn response(reader: &mut impl MessageReader) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = reader
                .read()
                .await
                .unwrap()
                .expect("response must flush before drain");
            if frame.get("requestId").is_some() {
                return frame;
            }
        }
    })
    .await
    .unwrap()
}
async fn count(database: &mut SqliteConnection, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM runtime_events WHERE kind = ?")
        .bind(kind)
        .fetch_one(database)
        .await
        .unwrap()
}
async fn canonical(database: &mut SqliteConnection) -> (Vec<String>, Vec<String>) {
    let events = sqlx::query_scalar("SELECT event_json FROM runtime_events ORDER BY sequence")
        .fetch_all(&mut *database)
        .await
        .unwrap();
    let questions =
        sqlx::query_scalar("SELECT record_json FROM interaction_requests ORDER BY request_id")
            .fetch_all(database)
            .await
            .unwrap();
    (events, questions)
}
