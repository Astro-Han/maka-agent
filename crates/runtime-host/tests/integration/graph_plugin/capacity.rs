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

use super::{
    ClientFixture, Host, LocalListener, Peer, Provider, Value, answer, configure, next, ready,
    toggle,
};
use maka_graph::Mode;
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn closed_history_releases_capacity_and_evicted_root_reopens_its_durable_epoch() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let fixture = ClientFixture::new("maka-graph-capacity-");
        let (provider, mut requests) = Provider::controlled().await;
        let model = configure(&fixture, &provider.base_url).await;
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("capacity.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-capacity-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop.clone()),
        );
        let mut peer = Peer::new(host.clone(), "graph-capacity").await;
        ready(&mut peer).await;
        let created = peer.rpc("session.create", json!({
            "sessionId":"template", "workspace":{"kind":"host_path","path":fixture.workspace},
            "modelTarget":{"kind":"explicit","connectionId":model.connection_id,
                "connectionSlug":model.connection_slug,"model":model.model},
            "orchestrationMode":"graph"
        })).await;
        assert_eq!(created["ok"], true, "{created}");
        toggle(&mut peer, true).await;
        ready(&mut peer).await;
        peer.close().await;
        stop.cancel();
        server.await.unwrap().unwrap();
        cleanup.disarm();
        drop(host);
        let log = std::sync::Arc::new(fixture.log().await);
        let repository = super::storage::repository(log.clone());
        let configuration = log
            .get_session::<Value>("template")
            .await
            .unwrap()
            .unwrap()
            .configuration;
        for index in 0..257 {
            let id = format!("history-{index:03}");
            log.create_session(&id, &id, &configuration, 1)
                .await
                .unwrap();
            let epoch = repository.open(&id, Mode::Graph, None, 2).await.unwrap();
            repository.stop(&id, &epoch.graph_id, None).await.unwrap();
        }
        drop(repository);
        std::sync::Arc::try_unwrap(log)
            .ok()
            .unwrap()
            .close()
            .await
            .unwrap();
        let host = Host::open(fixture.owner()).await.unwrap();
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop.clone()),
        );
        let mut peer = Peer::new(host.clone(), "graph-capacity-reopen").await;
        ready(&mut peer).await;
        toggle(&mut peer, false).await;
        ready(&mut peer).await;
        for index in 0..257 {
            super::approve(&mut peer, &format!("history-{index:03}")).await;
        }
        toggle(&mut peer, true).await;
        ready(&mut peer).await;
        toggle(&mut peer, false).await;
        ready(&mut peer).await;
        loop {
            let status = peer.rpc("host.diagnostics.query", json!({})).await;
            assert_eq!(status["ok"], true, "{status}");
            if !status["result"]["residencies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| {
                    entry["label"] == "plugin-background" && entry["count"].as_u64().unwrap() > 0
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Recovery visited more roots than the live capacity. Historical reads and
        // later admission must still work after those idle coordinators were retired.
        let evicted = "history-000";
        let started = peer
            .rpc(
                "turn.start",
                json!({
                    "sessionId":evicted, "turnId":"reopen", "content":{"text":"Continue"},
                    "turnOrchestration":{"mode":"swarm","source":"slash_command"}
                }),
            )
            .await;
        assert_eq!(started["ok"], true, "{started}");
        let request = next(&mut requests, "evicted graph supervisor").await;
        assert!(request.body.to_string().contains("Agent Swarm supervisor"));
        request.reply.send(answer("Reopened")).unwrap();
        loop {
            let state = peer
                .rpc("turn.query", json!({"sessionId":evicted,"turnId":"reopen"}))
                .await;
            if state["result"]["status"] == "completed" {
                break;
            }
            assert!(
                matches!(
                    state["result"]["status"].as_str(),
                    Some("admitted" | "created" | "running")
                ),
                "{state}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        peer.close().await;
        stop.cancel();
        server.await.unwrap().unwrap();
        cleanup.disarm();
        drop(host);
        let log = std::sync::Arc::new(fixture.log().await);
        let repository = super::storage::repository(log.clone());
        let epoch = repository
            .open(evicted, Mode::Swarm, None, 3)
            .await
            .unwrap();
        assert_eq!(
            epoch.epoch, 2,
            "an evicted closed epoch must roll forward, not return a cancelled submission"
        );
        drop(repository);
        std::sync::Arc::try_unwrap(log)
            .ok()
            .unwrap()
            .close()
            .await
            .unwrap();
    })
    .await
    .expect("closed history must not permanently exhaust graph capacity");
}
