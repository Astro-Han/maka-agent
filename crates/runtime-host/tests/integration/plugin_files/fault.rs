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

use crate::{
    javascript_plugins::{package, ready},
    support::{
        client_probe::ClientFixture,
        message_recovery::{Provider, configure},
        peer::Peer,
    },
};
use maka_event_log::root::ROOT_DATABASE;
use maka_runtime::event::{Fact, ToolOutcome};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::json;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn caught_service_error_cannot_hide_file_settlement_failure() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let fixture = ClientFixture::new("maka-plugin-files-fault-");
        let (provider, mut requests) = Provider::controlled().await;
        let model = configure(&fixture, &provider.base_url).await;
        let source = package(&fixture.workspace, "example.files", "shared", include_str!("../../fixtures/files-plugin.mjs"), false);
        let owner = fixture.owner();
        let database = owner.canonical_path().join(ROOT_DATABASE);
        let host = Host::open(owner).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("file-fault.sock");
        #[cfg(windows)]
        let endpoint = std::path::PathBuf::from(format!(r"\\.\pipe\maka-file-fault-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(LocalListener::bind(&endpoint).unwrap().serve(host.clone(), stop.clone()));
        let mut peer = Peer::new(host.clone(), "file-fault").await;
        ready(&mut peer).await;
        let installed = peer.rpc("plugin.package.install", json!({"sourcePath":source})).await;
        assert_eq!(installed["ok"], true, "{installed}"); ready(&mut peer).await;
        let created = peer.rpc("session.create", json!({
            "sessionId":"files", "workspace":{"kind":"host_path","path":fixture.workspace}, "permissionMode":"ask",
            "modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model}
        })).await;
        assert_eq!(created["ok"], true, "{created}");
        let mut faults = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(database)).await.unwrap();
        sqlx::raw_sql("CREATE TRIGGER fail_file_t2 BEFORE INSERT ON event_log
            WHEN NEW.kind = 'tool_settled' AND EXISTS (
                SELECT 1 FROM event_log WHERE operation_id = NEW.operation_id AND kind = 'tool_dispatched'
                AND json_extract(event_json, '$.fact.name') = 'Write')
            BEGIN SELECT RAISE(ABORT, 'file T2 fault'); END;")
            .execute(&mut faults).await.unwrap();
        let started = peer.rpc("turn.start", json!({"sessionId":"files","turnId":"fault","content":{"text":"verify"}})).await;
        assert_eq!(started["ok"], true, "{started}");
        for (name, input) in [("tool_search", json!({"query":"FilesForward"})), ("FilesForward", json!({}))] {
            let request = requests.recv().await.unwrap();
            request.reply.send(json!({"index":0,"delta":{"tool_calls":[{
                "index":0,"id":name,"type":"function","function":{"name":name,"arguments":input.to_string()}
            }]},"finish_reason":"tool_calls"})).unwrap();
        }
        // Uncertain durable outcome closes Host admission and drains its workers.
        server.await.unwrap().unwrap(); peer.close().await; drop(host); cleanup.disarm();
        sqlx::raw_sql("DROP TRIGGER fail_file_t2").execute(&mut faults).await.unwrap();
        faults.close().await.unwrap();
        assert_eq!(std::fs::read_to_string(fixture.workspace.join("fault.txt")).unwrap(), "effect happened");
        let log = fixture.log().await;
        let prefix = log.prefix(200, 1024 * 1024).await.unwrap();
        let outer = prefix.events.iter().find_map(|row| match &row.event.fact {
            Fact::ToolDispatched { name, operation_id, .. } if name == "FilesForward" => Some(operation_id),
            _ => None,
        }).unwrap();
        assert!(!prefix.events.iter().any(|row| matches!(&row.event.fact,
            Fact::ToolSettled { operation_id, outcome:ToolOutcome::Succeeded { .. } } if operation_id == outer
        )), "catching a service error fabricated a successful outer tool settlement");
        log.close().await.unwrap();
    }).await.expect("uncertain file effects must drain without a plugin promise deadlock");
}
