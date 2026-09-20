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
use maka_runtime::artifact::content_digest;
use maka_runtime_host::server::{Host, local::LocalListener};
use maka_transport::{MessageReader, MessageWriter};
use serde_json::{Value, json};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::support::execution_fixture as support;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn artifact_mutation_faults_flush_failure_drain_and_never_publish_or_replay_partial_changes()
{
    for cut in ["insert", "revision", "delete"] {
        let (directory, ns, root, provider, host) = support::fixture().await;
        drop(provider);
        let path = root.join(ROOT_DATABASE);
        let mut database =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
                .await
                .unwrap();
        let socket_path = directory.path().join("artifact.sock");
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
                &json!({"kind":"hello","clientInstanceId":"artifact-boundary",
            "protocolMin":0,"protocolMax":0,
            "compatibilityEpoch":maka_protocol::COMPATIBILITY_EPOCH,"compositionId":"maka.interactive"}),
            )
            .await
            .unwrap();
        assert_eq!(reader.read().await.unwrap().unwrap()["state"], "ready");
        let begin = json!({"kind":"begin","sessionId":"session","uploadId":"upload",
            "name":"file","mimeType":"text/plain","totalBytes":1,"contentSha256":content_digest(b"x")});
        assert_eq!(
            request(&mut reader, &mut writer, "artifact.ingest", begin).await["result"]["kind"],
            "upload_opened"
        );
        request(&mut reader, &mut writer, "artifact.ingest", json!({
            "kind":"chunk","sessionId":"session","uploadId":"upload","offset":0,"chunkBase64":"eA=="
        })).await;
        let commit = json!({"kind":"commit","sessionId":"session","uploadId":"upload"});
        let input = if cut == "delete" {
            let committed =
                request(&mut reader, &mut writer, "artifact.ingest", commit.clone()).await;
            assert_eq!(committed["result"]["kind"], "committed");
            json!({"sessionId":"session","artifactId":committed["result"]["attachment"]["ref"]["relativePath"]})
        } else {
            commit
        };
        sqlx::raw_sql(if cut == "insert" {
            "CREATE TRIGGER artifact_fault BEFORE INSERT ON artifacts
             BEGIN SELECT RAISE(ABORT, 'injected insert failure'); END;"
        } else {
            "CREATE TRIGGER artifact_fault BEFORE INSERT ON artifact_catalog
             BEGIN SELECT RAISE(ABORT, 'injected revision failure'); END;"
        })
        .execute(&mut database)
        .await
        .unwrap();
        let response = request(
            &mut reader,
            &mut writer,
            if cut == "delete" {
                "artifact.delete"
            } else {
                "artifact.ingest"
            },
            input,
        )
        .await;
        assert_eq!(response["error"]["code"], "persistence_failed");
        assert!(response.get("result").is_none());
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        drop(reader);
        drop(writer);
        let records: Vec<(String, Vec<u8>)> =
            sqlx::query_as("SELECT record_json, payload FROM artifacts")
                .fetch_all(&mut database)
                .await
                .unwrap();
        assert_eq!(records.len(), usize::from(cut == "delete"));
        if cut == "delete" {
            assert_eq!(records[0].1, b"x");
        }
        let revision: Option<i64> = sqlx::query_scalar(
            "SELECT revision FROM artifact_catalog WHERE session_id = 'session'",
        )
        .fetch_optional(&mut database)
        .await
        .unwrap();
        assert_eq!(revision, (cut == "delete").then_some(1));
        sqlx::raw_sql("DROP TRIGGER artifact_fault")
            .execute(&mut database)
            .await
            .unwrap();
        database.close().await.unwrap();
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
            &SqliteConnectOptions::new().filename(&path).read_only(true),
        )
        .await
        .unwrap();
        let after: Vec<(String, Vec<u8>)> =
            sqlx::query_as("SELECT record_json, payload FROM artifacts")
                .fetch_all(&mut database)
                .await
                .unwrap();
        assert_eq!(
            after, records,
            "{cut}: restart cannot replay a consumed upload or deletion"
        );
        let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_events")
            .fetch_one(&mut database)
            .await
            .unwrap();
        assert_eq!(events, 0);
        database.close().await.unwrap();
    }
}

async fn request(
    reader: &mut impl MessageReader,
    writer: &mut impl MessageWriter,
    operation: &str,
    input: Value,
) -> Value {
    writer
        .write(&json!({"requestId":"artifact-boundary","operation":operation,"input":input}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = reader
                .read()
                .await
                .unwrap()
                .expect("failure response must flush before drain");
            if frame.get("requestId").is_some() {
                return frame;
            }
        }
    })
    .await
    .unwrap()
}
