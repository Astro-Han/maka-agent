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

use super::support::{
    client_probe::ClientFixture,
    message_recovery::{Provider, configure},
    peer::Peer,
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
mod client;

pub(super) async fn converged(peer: &mut Peer) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = peer
                .rpc("plugin.platform.query", json!({"view":"status"}))
                .await;
            assert_eq!(state["ok"], true, "{state}");
            if state["result"]["convergence"] == "converged" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

pub(super) async fn disabled(peer: &mut Peer, disabled: bool) {
    let result = peer
        .rpc(
            "plugin.composition.apply",
            json!({
                "operations":[
                    {"type":"update","entryId":"maka.skills","patch":{"disabled":disabled}},
                    {"type":"update","entryId":"maka.skills.ui","patch":{"disabled":disabled}}
                ]
            }),
        )
        .await;
    assert_eq!(result["ok"], true, "{result}");
    converged(peer).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_skills_preserve_plain_chat_and_explicit_failure_across_host_restart() {
    let fixture = ClientFixture::new("maka-skills-plugin-");
    let skill = fixture.workspace.join(".maka/skills/review");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: Review\ndescription: Review code\n---\nReview with care.",
    )
    .unwrap();
    let provider = Provider::start().await;
    let model = configure(&fixture, &provider.base_url).await;
    let mut first_receipt = Value::Null;
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("skills.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-skills-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop),
        );
        let mut peer = Peer::new(host, "skills").await;
        converged(&mut peer).await;
        if !reopened {
            let created = peer.rpc("session.create", json!({
                "sessionId":"skills-session",
                "workspace":{"kind":"host_path","path":fixture.workspace},
                "modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model}
            })).await;
            assert_eq!(created["ok"], true, "{created}");
            let context = json!({"workspace":{"kind":"host_path","path":fixture.workspace}});
            let catalog = peer
                .rpc(
                    "skill.catalog.query",
                    json!({
                        "kind":"start","context":context,"view":"governance"
                    }),
                )
                .await;
            assert_eq!(catalog["ok"], true, "{catalog}");
            let revision = catalog["result"]["revision"].clone();
            let bundled = peer
                .rpc(
                    "skill.catalog.query",
                    json!({
                        "kind":"start","context":context,"view":"bundled"
                    }),
                )
                .await;
            assert_eq!(bundled["result"]["revision"], revision);
            let preview = peer
                .rpc(
                    "skill.catalog.preview-update",
                    json!({
                        "context":context,"expectedRevision":revision,"ref":"project:maka:review"
                    }),
                )
                .await;
            assert_eq!(preview["result"]["reason"], "not_managed", "{preview}");
            client::verify(&mut peer).await;
        }
        let catalog = peer
            .rpc(
                "skill.catalog.invocable.query",
                json!({
                    "kind":"start","target":{"kind":"session","sessionId":"skills-session"}
                }),
            )
            .await;
        assert_eq!(catalog["result"]["items"], json!([]), "{catalog}");
        let governance = peer.rpc("skill.catalog.query", json!({
            "kind":"start","context":{"workspace":{"kind":"host_path","path":fixture.workspace}},"view":"governance"
        })).await;
        assert_eq!(governance["ok"], false, "{governance}");
        assert_eq!(
            governance["error"]["code"], "operation_unavailable",
            "{governance}"
        );
        let blocked = peer
            .rpc(
                "turn.start",
                json!({
                    "sessionId":"skills-session","turnId":format!("explicit-{reopened}"),
                    "content":{"text":"Please review"},"skillIds":["review"]
                }),
            )
            .await;
        assert_eq!(blocked["result"]["kind"], "blocked", "{blocked}");
        let input = json!({
            "sessionId":"skills-session","turnId":"ordinary-chat","content":{"text":"Ordinary chat"}
        });
        let started = tokio::time::timeout(
            Duration::from_secs(5),
            peer.rpc("turn.start", input.clone()),
        )
        .await
        .unwrap();
        assert_eq!(started["result"]["kind"], "started", "{started}");
        if reopened {
            assert_eq!(
                started["result"]["turn"]["runId"],
                first_receipt["result"]["turn"]["runId"]
            );
        } else {
            first_receipt = started;
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let turn = peer
                    .rpc(
                        "turn.query",
                        json!({"sessionId":"skills-session","turnId":"ordinary-chat"}),
                    )
                    .await;
                assert_eq!(turn["ok"], true, "{turn}");
                match turn["result"]["status"].as_str() {
                    Some("completed") => break,
                    Some("failed" | "cancelled") => panic!("{turn}"),
                    _ => tokio::task::yield_now().await,
                }
            }
        })
        .await
        .unwrap();
        if reopened {
            disabled(&mut peer, false).await;
            let governance = peer.rpc("skill.catalog.query", json!({
                "kind":"start","context":{"workspace":{"kind":"host_path","path":fixture.workspace}},"view":"governance"
            })).await;
            let review = governance["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["ref"] == "project:maka:review")
                .unwrap();
            assert_eq!(
                review["pinned"], true,
                "preference survives plugin retirement and Host restart"
            );
            let catalog = peer
                .rpc(
                    "skill.catalog.invocable.query",
                    json!({
                        "kind":"start","target":{"kind":"session","sessionId":"skills-session"}
                    }),
                )
                .await;
            assert!(
                catalog["result"]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item["id"] == "review"),
                "{catalog}"
            );
        }
        peer.close().await;
        drop(cleanup);
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    let home = fixture.workspace.parent().unwrap().join("client-home");
    std::fs::create_dir(&home).unwrap();
    std::fs::write(fixture.workspace.join("import-client.md"),
        "---\nname: Imported Client Source\ndescription: Imported through the Client plugin\n---\nInstructions.\n").unwrap();
    fixture
        .run_with_options(
            "--skills-client-workspace",
            false,
            "skills-client-bundle-accepted",
            maka_runtime_host::server::HostOptions {
                skill_home: Some(home),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        1,
        "blocked input and replay cannot call a model"
    );
}
