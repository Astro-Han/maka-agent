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
    skills_plugin::{converged, disabled},
    support::{client_probe::ClientFixture, peer::Peer},
};
use maka_runtime::artifact::content_digest;
use maka_runtime_host::server::{Host, HostOptions, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

async fn catalog(peer: &mut Peer, context: &Value, view: &str) -> Value {
    let value = peer
        .rpc(
            "skill.catalog.query",
            json!({"kind":"start","context":context,"view":view}),
        )
        .await;
    assert_eq!(value["ok"], true, "{value}");
    value["result"].clone()
}
async fn mutate(peer: &mut Peer, context: &Value, mutation: Value) -> Value {
    let basis = catalog(peer, context, "governance").await;
    let value = peer
        .rpc(
            "skill.catalog.mutate",
            json!({
                "context":context,"expectedRevision":basis["revision"],"mutation":mutation
            }),
        )
        .await;
    assert_eq!(value["ok"], true, "{value}");
    value["result"].clone()
}
fn document(body: &str) -> String {
    format!("---\nname: Review\ndescription: Review code\n---\n{body}\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skill_publication_and_confirmed_update_work_through_host_without_a_model() {
    let fixture = ClientFixture::new("maka-skill-management-");
    let home = fixture.workspace.parent().unwrap().join("home");
    let source = home.join(".maka/skill-sources/review");
    std::fs::create_dir_all(&home).unwrap();
    let original = document("Original");
    let updated = document("Updated");
    let local = document("Local edit");
    let import_file = fixture.workspace.join("review.md");
    std::fs::write(&import_file, &original).unwrap();
    for (base, id) in [(".maka", "user-review"), (".agents", "agent-review")] {
        let directory = home.join(base).join("skills").join(id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("SKILL.md"), &original).unwrap();
    }
    let context = json!({"workspace":{"kind":"host_path","path":fixture.workspace}});
    for reopened in [false, true] {
        let owner = fixture.owner();
        let root = owner.canonical_path().to_owned();
        let installed = root.join("skills/review");
        let host = Host::open_with_options(
            owner,
            None,
            HostOptions {
                skill_home: Some(home.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("management.sock");
        #[cfg(windows)]
        let endpoint = std::path::PathBuf::from(format!(
            r"\\.\pipe\maka-management-{}",
            uuid::Uuid::new_v4()
        ));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop),
        );
        let mut peer = Peer::new(host, "management").await;
        converged(&mut peer).await;
        if !reopened {
            let imported = peer
                .rpc("skill.source.import", json!({"sourcePath":import_file}))
                .await;
            assert_eq!(imported["result"]["kind"], "imported", "{imported}");
            assert_eq!(imported["result"]["source"]["id"], "review");
            assert_eq!(
                std::fs::read(source.join("SKILL.md")).unwrap(),
                original.as_bytes()
            );
            let duplicate = peer
                .rpc("skill.source.import", json!({"sourcePath":import_file}))
                .await;
            assert_eq!(
                duplicate["result"]["reason"], "already_exists",
                "{duplicate}"
            );
            let invalid = fixture.workspace.join("invalid.md");
            std::fs::write(&invalid, "not a Skill").unwrap();
            let rejected = peer
                .rpc("skill.source.import", json!({"sourcePath":invalid}))
                .await;
            assert_eq!(rejected["result"]["reason"], "invalid_skill", "{rejected}");
            assert!(!home.join(".maka/skill-sources/invalid").exists());
            for (base, id, reference) in [
                (".maka", "user-review", "user:maka:user-review"),
                (".agents", "agent-review", "user:agents:agent-review"),
            ] {
                let directory = home.join(base).join("skills").join(id);
                let resolved = peer
                    .rpc(
                        "skill.catalog.resolve-path",
                        json!({
                            "context":context, "ref":reference, "target":"file"
                        }),
                    )
                    .await;
                assert_eq!(resolved["result"]["kind"], "resolved", "{resolved}");
                let path = std::path::Path::new(resolved["result"]["path"].as_str().unwrap());
                assert_eq!(path, directory.join("SKILL.md").canonicalize().unwrap());
                let removed = mutate(
                    &mut peer,
                    &context,
                    json!({"kind":"delete","ref":reference}),
                )
                .await;
                assert_eq!(removed["kind"], "committed", "{removed}");
                assert!(!directory.exists());
                let missing = peer
                    .rpc(
                        "skill.catalog.resolve-path",
                        json!({
                            "context":context, "ref":reference, "target":"directory"
                        }),
                    )
                    .await;
                assert_eq!(missing["result"]["reason"], "missing", "{missing}");
            }
            let bundled = catalog(&mut peer, &context, "bundled").await;
            let result = peer.rpc("skill.catalog.mutate", json!({
                "context":context, "expectedRevision":bundled["revision"],
                "mutation":{"kind":"install","sourceType":"bundled","sourceId":"computer-use"}
            })).await;
            assert_eq!(
                result["result"]["entry"]["sourceType"], "bundled",
                "{result}"
            );
            assert_eq!(result["result"]["entry"]["manageable"], true);
            let starter = mutate(&mut peer, &context, json!({"kind":"create_starter"})).await;
            assert_eq!(starter["kind"], "committed", "{starter}");
            let again = mutate(&mut peer, &context, json!({"kind":"create_starter"})).await;
            assert_eq!(again["kind"], "unchanged", "{again}");
            assert_eq!(again["entry"]["ref"], starter["entry"]["ref"]);
            let result = mutate(
                &mut peer,
                &context,
                json!({"kind":"install","sourceType":"managed","sourceId":"review"}),
            )
            .await;
            assert_eq!(
                result["entry"]["managedUpdateStatus"], "up_to_date",
                "{result}"
            );
            assert_eq!(
                std::fs::read(installed.join("SKILL.md")).unwrap(),
                original.as_bytes()
            );
            std::fs::write(installed.join("notes.txt"), "keep my resource").unwrap();
            std::fs::write(installed.join("SKILL.md"), &local).unwrap();
            std::fs::write(source.join("SKILL.md"), &updated).unwrap();
            let auto = mutate(
                &mut peer,
                &context,
                json!({
                    "kind":"update_managed","ref":"workspace:legacy:review","force":false,
                    "expectedCurrentSha256":null,"expectedSourceSha256":null
                }),
            )
            .await;
            assert_eq!(auto["reason"], "local_modified", "{auto}");
            let basis = catalog(&mut peer, &context, "governance").await;
            let preview = peer.rpc("skill.catalog.preview-update", json!({
                "context":context,"expectedRevision":basis["revision"],"ref":"workspace:legacy:review"
            })).await;
            assert_eq!(
                preview["result"]["expectedCurrentSha256"],
                content_digest(local.as_bytes()),
                "{preview}"
            );
            assert_eq!(
                preview["result"]["expectedSourceSha256"],
                content_digest(updated.as_bytes()),
                "{preview}"
            );
            let stale = mutate(
                &mut peer,
                &context,
                json!({
                    "kind":"update_managed","ref":"workspace:legacy:review","force":true,
                    "expectedCurrentSha256":content_digest(original.as_bytes()),
                    "expectedSourceSha256":preview["result"]["expectedSourceSha256"]
                }),
            )
            .await;
            assert_eq!(stale["reason"], "source_changed", "{stale}");
            let changed = mutate(
                &mut peer,
                &context,
                json!({
                    "kind":"update_managed","ref":"workspace:legacy:review","force":true,
                    "expectedCurrentSha256":preview["result"]["expectedCurrentSha256"],
                    "expectedSourceSha256":preview["result"]["expectedSourceSha256"]
                }),
            )
            .await;
            assert_eq!(changed["kind"], "committed", "{changed}");
            assert_eq!(
                changed["entry"]["managedUpdateStatus"], "up_to_date",
                "{changed}"
            );
            assert_eq!(
                std::fs::read(installed.join("SKILL.md")).unwrap(),
                updated.as_bytes()
            );
            assert_eq!(
                std::fs::read(installed.join(".maka/baseline/SKILL.md")).unwrap(),
                updated.as_bytes()
            );
            assert_eq!(
                std::fs::read(installed.join("notes.txt")).unwrap(),
                b"keep my resource"
            );
            disabled(&mut peer, true).await;
        } else {
            disabled(&mut peer, false).await;
            let page = catalog(&mut peer, &context, "governance").await;
            let review = page["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["ref"] == "workspace:legacy:review")
                .unwrap();
            assert_eq!(review["managedUpdateStatus"], "up_to_date");
            let removed = mutate(
                &mut peer,
                &context,
                json!({"kind":"delete","ref":"workspace:legacy:review"}),
            )
            .await;
            assert_eq!(removed["kind"], "committed", "{removed}");
            assert!(removed["entry"].is_null());
            assert!(!installed.exists());
            assert!(
                source.join("SKILL.md").exists(),
                "uninstall cannot delete the source library"
            );
        }
        assert!(!root.join("skill-transactions").exists());
        for journal in [
            ".maka/.skills-publication",
            ".agents/.skills-publication",
            ".maka/.skill-sources-publication",
        ] {
            assert_eq!(
                std::fs::read_dir(home.join(journal).join("transactions"))
                    .unwrap()
                    .count(),
                0
            );
        }
        let namespace = std::fs::read_dir(root.join("plugin-data"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            std::fs::read_dir(namespace.join("transactions"))
                .unwrap()
                .count(),
            0
        );
        peer.close().await;
        drop(cleanup);
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
