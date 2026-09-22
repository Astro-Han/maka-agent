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
    super::skills_plugin::client::workspace(
        peer,
        &context["workspace"]["path"],
        json!({"kind":"catalog","view":view}),
    )
    .await
}
async fn mutate(peer: &mut Peer, context: &Value, mutation: Value) -> Value {
    let basis = catalog(peer, context, "governance").await;
    super::skills_plugin::client::workspace(
        peer,
        &context["workspace"]["path"],
        json!({"kind":"mutate","expectedRevision":basis["revision"],"mutation":mutation}),
    )
    .await
}
fn document(body: &str) -> String {
    format!("---\nname: Review\ndescription: Review code\n---\n{body}\n")
}
async fn approve_user(peer: &mut Peer) -> Value {
    let status =
        super::skills_plugin::client::request(peer, "user-authorization", json!({"kind":"status"}))
            .await;
    super::skills_plugin::client::authorization(
        peer,
        json!({
            "kind":"approve","request":{
                "operationId":uuid::Uuid::new_v4(),"title":"Manage user Skills",
                "target":status["target"],"capabilities":["read_files","write_files"]
            }
        }),
    )
    .await["grant"]["id"]
        .clone()
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
    let mut approved = Value::Null;
    let mut pending_recovery = None::<std::path::PathBuf>;
    for reopened in [false, true] {
        let owner = fixture.owner();
        let root = owner.canonical_path().to_owned();
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
        if reopened {
            disabled(&mut peer, false).await;
        }
        let namespace = std::fs::read_dir(root.join("plugin-data"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.join("skills").is_dir())
            .unwrap();
        let installed = namespace.join("skills/review");
        if reopened {
            let page = catalog(&mut peer, &context, "governance").await;
            assert!(page["userRecovery"].is_string(), "{page}");
            let pending = pending_recovery.as_ref().unwrap();
            assert!(
                pending.join("proof").is_file(),
                "revoked consent must leave pending files untouched"
            );
            approved = approve_user(&mut peer).await;
            let status = super::skills_plugin::client::request(
                &mut peer,
                "user-authorization",
                json!({"kind":"recover","grant":approved}),
            )
            .await;
            assert!(status["recovery"].is_null(), "{status}");
            assert!(
                !pending.exists(),
                "re-consent resumes the same durable publication"
            );
        }
        if !reopened {
            approved = approve_user(&mut peer).await;
            let grant = approved.clone();
            let imported = super::skills_plugin::client::request(
                &mut peer,
                "import-source",
                json!({"sourcePath":import_file,"grant":grant}),
            )
            .await;
            assert_eq!(imported["kind"], "imported", "{imported}");
            assert_eq!(imported["source"]["id"], "review");
            assert_eq!(
                std::fs::read(source.join("SKILL.md")).unwrap(),
                original.as_bytes()
            );
            let duplicate = super::skills_plugin::client::request(
                &mut peer,
                "import-source",
                json!({"sourcePath":import_file,"grant":grant}),
            )
            .await;
            assert_eq!(duplicate["reason"], "already_exists", "{duplicate}");
            let invalid = fixture.workspace.join("invalid.md");
            std::fs::write(&invalid, "not a Skill").unwrap();
            let rejected = super::skills_plugin::client::request(
                &mut peer,
                "import-source",
                json!({"sourcePath":invalid,"grant":grant}),
            )
            .await;
            assert_eq!(rejected["reason"], "invalid_skill", "{rejected}");
            assert!(!home.join(".maka/skill-sources/invalid").exists());
            for (base, id, reference) in [
                (".maka", "user-review", "user:maka:user-review"),
                (".agents", "agent-review", "user:agents:agent-review"),
            ] {
                let directory = home.join(base).join("skills").join(id);
                let resolved = super::skills_plugin::client::workspace(
                    &mut peer,
                    &context["workspace"]["path"],
                    json!({
                        "kind":"resolve_path","ref":reference, "target":"file"
                    }),
                )
                .await;
                assert_eq!(resolved["kind"], "resolved", "{resolved}");
                let path = std::path::Path::new(resolved["path"].as_str().unwrap());
                assert_eq!(
                    path.canonicalize().unwrap(),
                    directory.join("SKILL.md").canonicalize().unwrap()
                );
                let basis = catalog(&mut peer, &context, "governance").await;
                let removed = super::skills_plugin::client::request(&mut peer, "user-request", json!({
                    "workspace":{"workspace":context["workspace"],"sandboxMode":"workspace-write","collaborationMode":"agent"},
                    "request":{"kind":"mutate","expectedRevision":basis["revision"],"grant":grant,
                        "mutation":{"kind":"delete","ref":reference}}
                })).await;
                assert_eq!(removed["kind"], "committed", "{removed}");
                assert!(!directory.exists());
                let missing = super::skills_plugin::client::workspace(
                    &mut peer,
                    &context["workspace"]["path"],
                    json!({
                        "kind":"resolve_path","ref":reference, "target":"directory"
                    }),
                )
                .await;
                assert_eq!(missing["reason"], "missing", "{missing}");
            }
            let bundled = catalog(&mut peer, &context, "bundled").await;
            let result = super::skills_plugin::client::workspace(
                &mut peer,
                &context["workspace"]["path"],
                json!({
                    "kind":"mutate","expectedRevision":bundled["revision"],
                    "mutation":{"kind":"install","sourceType":"bundled","sourceId":"computer-use"}
                }),
            )
            .await;
            assert_eq!(result["entry"]["sourceType"], "bundled", "{result}");
            assert_eq!(result["entry"]["manageable"], true);
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
            let preview = super::skills_plugin::client::workspace(&mut peer, &context["workspace"]["path"], json!({
                "kind":"preview","expectedRevision":basis["revision"],"ref":"workspace:legacy:review"
            })).await;
            assert_eq!(
                preview["expectedCurrentSha256"],
                content_digest(local.as_bytes()),
                "{preview}"
            );
            assert_eq!(
                preview["expectedSourceSha256"],
                content_digest(updated.as_bytes()),
                "{preview}"
            );
            let stale = mutate(
                &mut peer,
                &context,
                json!({
                    "kind":"update_managed","ref":"workspace:legacy:review","force":true,
                    "expectedCurrentSha256":content_digest(original.as_bytes()),
                    "expectedSourceSha256":preview["expectedSourceSha256"]
                }),
            )
            .await;
            assert_eq!(stale["reason"], "source_changed", "{stale}");
            let changed = mutate(
                &mut peer,
                &context,
                json!({
                    "kind":"update_managed","ref":"workspace:legacy:review","force":true,
                    "expectedCurrentSha256":preview["expectedCurrentSha256"],
                    "expectedSourceSha256":preview["expectedSourceSha256"]
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
        } else {
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
            let publications = std::fs::read_dir(home.join(journal))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            assert_eq!(
                publications.len(),
                1,
                "each Host owns its recovery namespace"
            );
            assert_eq!(
                std::fs::read_dir(publications[0].join("transactions"))
                    .unwrap()
                    .count(),
                0
            );
        }
        assert_eq!(
            std::fs::read_dir(namespace.join("transactions"))
                .unwrap()
                .count(),
            0
        );
        assert!(
            !home.join(".maka-workspace.json").exists(),
            "file consent must not initialize an execution workspace"
        );
        if !reopened {
            let journal = std::fs::read_dir(home.join(".maka/.skill-sources-publication"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path()
                .join("transactions");
            let digest = content_digest(b"interrupted collection");
            let pending = journal.join(format!("gc-{}-{}", uuid::Uuid::new_v4(), &digest[7..]));
            std::fs::create_dir(&pending).unwrap();
            std::fs::write(pending.join("proof"), b"accepted publication").unwrap();
            pending_recovery = Some(pending);
            super::skills_plugin::client::authorization(
                &mut peer,
                json!({"kind":"revoke","id":approved}),
            )
            .await;
            disabled(&mut peer, true).await;
        }
        peer.close().await;
        drop(cleanup);
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
