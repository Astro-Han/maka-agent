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
    javascript_plugins::ready,
    support::{client_probe::ClientFixture, peer::Peer},
};
use maka_plugins::{client::Bundle, kernel::Definition};
use maka_runtime_host::{
    plugins::Setup,
    server::{Host, HostOptions, local::LocalListener},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

const ID: &str = "z.statistics";

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn renamed_insights_uses_public_remote_capabilities_and_preserves_state_across_retirement() {
    tokio::time::timeout(Duration::from_secs(40), scenario())
        .await
        .unwrap();
}

async fn scenario() {
    let fixture = ClientFixture::new("maka-insights-");
    let preferences = json!({"range":"30d","tab":"activity","selection":{"search":"🦀"}});
    let mut old_cursor = Value::Null;
    for reopened in [false, true] {
        let host = Host::open_with_options(
            fixture.owner(),
            None,
            HostOptions {
                plugins: setup(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("insights.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-insights-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop),
        );
        let mut peer = Peer::new(host, "insights-client").await;
        ready(&mut peer).await;
        let mut call = bind(&mut peer).await;
        let snapshot = invoke(&mut peer, &call, json!({"kind":"preferences"})).await;
        if reopened {
            assert_eq!(snapshot["snapshot"]["preferences"]["tab"], "activity");
            assert_eq!(
                snapshot["snapshot"]["preferences"]["selection"]["search"],
                "🦀"
            );
            let expired = invoke(
                &mut peer,
                &call,
                json!({"kind":"summary",
                "operationId":"00000000-0000-4000-8000-000000000001","cursor":old_cursor}),
            )
            .await;
            assert_eq!(expired["kind"], "refresh_required");
            let rates = invoke(
                &mut peer,
                &call,
                json!({"kind":"prices","query":{"kind":"start"}}),
            )
            .await;
            assert!(rates["page"]["entries"].as_array().unwrap().iter().any(
                |entry| entry["pricing"]["modelKey"] == "00-insights-fixture"
                    && entry["source"] == "custom"
            ));
        } else {
            assert!(snapshot["snapshot"]["revision"].is_null());
            let saved = invoke(
                &mut peer,
                &call,
                json!({"kind":"save_preferences",
                "expectedRevision":null,"preferences":preferences}),
            )
            .await;
            assert!(saved["snapshot"]["revision"].as_u64().unwrap() > 0);
            let conflict = invoke(
                &mut peer,
                &call,
                json!({"kind":"save_preferences",
                "expectedRevision":null,"preferences":preferences}),
            )
            .await;
            assert_eq!(conflict["kind"], "refresh_required");
            let rates = invoke(
                &mut peer,
                &call,
                json!({"kind":"prices","query":{"kind":"start"}}),
            )
            .await;
            let revision = rates["page"]["revision"].clone();
            let mutation = json!({"kind":"update_price","operationId":"00000000-0000-4000-8000-000000000002",
                "update":{"expectedRevision":revision,"mutation":{"kind":"upsert","pricing":{
                    "modelKey":"00-insights-fixture","inputUsdPer1M":0,"outputUsdPer1M":2}}}});
            let saved = invoke(&mut peer, &call, mutation.clone()).await;
            assert_eq!(saved["receipt"]["kind"], "committed");
            let conflict = invoke(&mut peer, &call, mutation).await;
            assert_eq!(conflict["receipt"]["kind"], "revision_conflict");
        }
        let activity = invoke(
            &mut peer,
            &call,
            json!({"kind":"activity","operationId":"00000000-0000-4000-8000-000000000003",
            "read":{"kind":"start","filter":{"from":0,"to":2000000000000_f64}}}),
        )
        .await;
        old_cursor = activity["page"]["cursor"].clone();
        assert!(old_cursor.is_string());
        let summary = invoke(
            &mut peer,
            &call,
            json!({"kind":"summary","operationId":"00000000-0000-4000-8000-000000000004",
            "cursor":old_cursor}),
        )
        .await;
        assert_eq!(summary["summary"]["models"]["calls"], 0);
        if !reopened {
            success(
                peer.rpc(
                    "plugin.composition.apply",
                    json!({"operations":[
                        {"type":"update","entryId":"statistics-backend","patch":{"disabled":true}}
                    ]}),
                )
                .await,
            );
            loop {
                let clients = success(
                    peer.rpc("plugin.client.query", json!({"kind":"snapshot"}))
                        .await,
                );
                if clients["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|entry| entry["extensionId"] != ID)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            let mut stale = call.clone();
            stale["input"] = json!({"kind":"preferences"});
            assert_eq!(peer.rpc("plugin.remote", stale).await["ok"], false);
            success(
                peer.rpc(
                    "plugin.composition.apply",
                    json!({"operations":[
                        {"type":"update","entryId":"statistics-backend","patch":{"disabled":false}}
                    ]}),
                )
                .await,
            );
            ready(&mut peer).await;
            let old_document = call["document"].clone();
            call = bind(&mut peer).await;
            let restored = invoke(&mut peer, &call, json!({"kind":"preferences"})).await;
            assert_eq!(
                restored["snapshot"]["preferences"]["selection"]["search"],
                "🦀"
            );
            let same_facts = invoke(
                &mut peer,
                &call,
                json!({"kind":"summary","operationId":"00000000-0000-4000-8000-000000000005",
                "cursor":old_cursor}),
            )
            .await;
            assert_eq!(
                same_facts, summary,
                "plugin retirement does not retire canonical accounting"
            );
            success(
                peer.rpc(
                    "plugin.remote",
                    json!({"kind":"close_document","document":old_document}),
                )
                .await,
            );
        }
        success(
            peer.rpc(
                "plugin.remote",
                json!({"kind":"close_document","document":call["document"]}),
            )
            .await,
        );
        peer.close().await;
        drop(cleanup);
        server.await.unwrap().unwrap();
    }
}

fn setup() -> Setup {
    Setup {
        builtins: [(ID.into(), Arc::new(Definition {
            id: ID.into(), revision: "binary".into(), dependencies: vec![], inject: vec![],
            plugin: Arc::new(maka_insights::plugin::Builtin {
                client: Bundle::builtin(ID, "binary", "export default function() {}").unwrap(),
            }),
        }))].into(),
        layers: [(ID.into(), serde_json::from_value(json!([
            {"type":"remove","entryId":"maka.insights.ui"},
            {"type":"remove","entryId":"maka.insights"},
            {"type":"insert","rootId":"profile","entry":{"id":"statistics-backend","packageId":ID}},
            {"type":"insert","rootId":"desktop-ui","entry":{"id":"statistics-ui","packageId":ID,
                "inject":[maka_insights::plugin::client_service(ID)]}}
        ])).unwrap())].into(),
        ..Default::default()
    }
}
async fn bind(peer: &mut Peer) -> Value {
    let clients = success(
        peer.rpc("plugin.client.query", json!({"kind":"snapshot"}))
            .await,
    );
    let entry = clients["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == ID)
        .unwrap();
    let binding = json!({"method":"request","sessionId":null,"client":{
        "entryId":entry["entryId"],"extensionId":entry["extensionId"],"activation":entry["activation"],
        "contentDigest":entry["contentDigest"],"clientDigest":entry["clientDigest"]}});
    let target = success(
        peer.rpc("plugin.remote", json!({"kind":"bind","binding":binding}))
            .await,
    )["target"]
        .clone();
    let document = success(
        peer.rpc("plugin.remote", json!({"kind":"open_document"}))
            .await,
    )["document"]
        .clone();
    json!({"kind":"call","binding":binding,"target":target,"document":document})
}
async fn invoke(peer: &mut Peer, call: &Value, input: Value) -> Value {
    let mut request = call.clone();
    request["input"] = input;
    success(peer.rpc("plugin.remote", request).await)["value"].clone()
}
fn success(response: Value) -> Value {
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}
