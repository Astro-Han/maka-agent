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

use super::*;
use maka_protocol::session::*;
use serde_json::json;

#[test]
fn session_management_cas_and_archive_preserve_identity_and_composer() {
    let directory = tempfile::tempdir().unwrap();
    let mut host = super::super::candidate::CandidateFixture::new(directory.path().join("root"));
    host.child = Some(
        Command::new(env!("CARGO_BIN_EXE_maka"))
            .args(["host", "serve", "--root"])
            .arg(&host.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    host.wait_for_registration();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = runtime.block_on(async {
        let client = support::model_client(&host.root, "http://127.0.0.1:9/v1").await;
        for (id, name) in [("managed", "Original session"), ("neighbor", "Untouched neighbor")] {
            client.create_session(decode_session_create_input(&json!({
                "sessionId":id, "name":name, "workspace":{"kind":"host_path", "path":directory.path()},
                "modelTarget":{"kind":"default"}
            })).unwrap()).await.unwrap();
        }
        client
    });
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Original session");
    tui.click_text("Original session");
    tui.wait_for("Message…");
    tui.send(b"draft remains here");
    tui.filter_command("Rename session");
    tui.click_text("Rename session");
    tui.wait_for("Cancel");
    tui.send("\x1b[200~我的新名称\x1b[201~".as_bytes());
    runtime.block_on(async {
        let current = client.session("managed").await.unwrap().unwrap();
        assert!(matches!(
            client
                .update_session_metadata(SessionMetadataUpdateInput {
                    session_id: current.id,
                    expected_revision: current.revision,
                    patch: SessionMetadataPatch {
                        name: Some("Changed elsewhere".into()),
                        labels: None,
                        is_flagged: None
                    },
                })
                .await
                .unwrap(),
            SessionUpdateResult::Committed { .. }
        ));
    });
    tui.send(b"\r");
    tui.wait_for("This session changed elsewhere.");
    assert!(tui.screen.snapshot().unwrap().screen.contains("我的新名称"));
    tui.send(b"\r\x1b"); // Disabled Save cannot overwrite the concurrent writer.
    tui.wait_until(|text| text.contains("Changed elsewhere") && !text.contains("Cancel"));
    tui.filter_command("Rename session");
    tui.click_text("Rename session");
    tui.wait_for("Cancel");
    tui.send("\x1b[200~中文会话\x1b[201~".as_bytes());
    tui.click_last_text("Save");
    tui.wait_until(|text| text.contains("中文会话") && !text.contains("Cancel"));
    assert!(
        tui.screen
            .snapshot()
            .unwrap()
            .screen
            .contains("draft remains here")
    );
    tui.filter_command("Archive session");
    tui.click_text("Archive session");
    tui.wait_for("History is kept.");
    tui.send(b"\r"); // Default focus is Cancel.
    tui.wait_until(|text| !text.contains("Cancel") && text.contains("draft remains here"));
    assert!(
        !runtime
            .block_on(client.session("managed"))
            .unwrap()
            .unwrap()
            .is_archived
    );
    tui.filter_command("Archive session");
    tui.click_text("Archive session");
    tui.wait_for("History is kept.");
    tui.send(b"\t\r");
    tui.wait_until(|text| !text.contains("Cancel") && text.contains("draft remains here"));
    assert!(
        runtime
            .block_on(client.session("managed"))
            .unwrap()
            .unwrap()
            .is_archived
    );
    // Workspace keeps archived rows discoverable; restore is an explicit state, not a toggle.
    tui.send(b"\x1b[1;3D");
    tui.wait_for("中文会话 · Archived");
    tui.click_text("中文会话 · Archived");
    tui.wait_for("draft remains here");
    tui.filter_command("Restore session");
    tui.click_text("Restore session");
    tui.wait_for("History is kept.");
    tui.send(b"\t\r");
    tui.wait_until(|text| !text.contains("Cancel") && text.contains("draft remains here"));
    runtime.block_on(async {
        let current = client.session("managed").await.unwrap().unwrap();
        assert!(!current.is_archived);
        assert_eq!(current.name, "中文会话");
        assert_eq!(
            client.session("neighbor").await.unwrap().unwrap().name,
            "Untouched neighbor"
        );
    });
    let moved = directory.path().join("中文 workspace");
    std::fs::create_dir(&moved).unwrap();
    tui.filter_command("Change workspace");
    tui.click_text("Change workspace");
    tui.wait_for("Absolute directory on the Host.");
    tui.send(format!("\x1b[200~{}/missing\x1b[201~\r", directory.path().display()).as_bytes());
    tui.wait_for("Enter an existing absolute directory on the Host.");
    let original = runtime
        .block_on(client.session("managed"))
        .unwrap()
        .unwrap();
    assert_eq!(
        original.workspace.host_cwd,
        directory.path().to_str().unwrap()
    );
    tui.send(format!("\x01\x1b[200~{}\x1b[201~", moved.display()).as_bytes());
    tui.wait_for("中文 workspace");
    tui.wait_until(|text| {
        !text.contains("Enter an existing absolute directory on the Host.")
            && text.contains("Absolute directory on the Host.")
    });
    tui.click_last_text("Switch");
    tui.wait_until(|text| !text.contains("Cancel") && text.contains("draft remains here"));
    runtime.block_on(async {
        let current = client.session("managed").await.unwrap().unwrap();
        assert_eq!(current.workspace.host_cwd, moved.to_str().unwrap());
        assert!(current.revision > original.revision);
        assert_eq!(
            client
                .session("neighbor")
                .await
                .unwrap()
                .unwrap()
                .workspace
                .host_cwd,
            directory.path().to_str().unwrap()
        );
    });
    // Switching an existing session selects a project identity, not a client-side path.
    let project_path = directory.path().join("Selected project");
    std::fs::create_dir(&project_path).unwrap();
    let project = runtime
        .block_on(
            client.mutate_project(maka_protocol::project::Mutation::Register {
                path: project_path.to_str().unwrap().into(),
                prefer: None,
            }),
        )
        .unwrap();
    tui.filter_command("Change project");
    tui.click_text("Change project");
    tui.wait_for("Selected project");
    tui.send(b"\r"); // Opening the chooser does not implicitly select the first row.
    tui.wait_for("Use project");
    assert_eq!(
        runtime
            .block_on(client.session("managed"))
            .unwrap()
            .unwrap()
            .workspace
            .host_cwd,
        moved.to_str().unwrap()
    );
    tui.click_text("Selected project");
    tui.wait_for("› Selected project"); // Observe selection before the independent catalog mutation.
    runtime
        .block_on(
            client.mutate_project(maka_protocol::project::Mutation::Archive {
                project_id: project.id.clone(),
            }),
        )
        .unwrap();
    tui.wait_for("Selected project · Archived");
    tui.send(b"\r"); // A stale or archived selection cannot dispatch a write.
    tui.wait_for("Use project");
    runtime
        .block_on(
            client.mutate_project(maka_protocol::project::Mutation::Restore {
                project_id: project.id.clone(),
            }),
        )
        .unwrap();
    tui.wait_until(|text| {
        text.contains("Selected project") && !text.contains("Selected project · Archived")
    });
    runtime.block_on(async {
        let current = client.session("managed").await.unwrap().unwrap();
        client
            .update_session_metadata(SessionMetadataUpdateInput {
                session_id: current.id,
                expected_revision: current.revision,
                patch: SessionMetadataPatch {
                    name: Some("Concurrent project edit".into()),
                    labels: None,
                    is_flagged: None,
                },
            })
            .await
            .unwrap();
    });
    tui.click_last_text("Use project");
    tui.wait_for("This session changed elsewhere.");
    tui.send(b"\x1b");
    tui.wait_until(|text| text.contains("Concurrent project edit") && !text.contains("Cancel"));
    tui.filter_command("Change project");
    tui.click_text("Change project");
    tui.wait_for("Selected project");
    tui.click_text("Selected project");
    tui.click_last_text("Use project");
    tui.wait_until(|text| !text.contains("Cancel") && text.contains("draft remains here"));
    runtime.block_on(async {
        let current = client.session("managed").await.unwrap().unwrap();
        assert_eq!(
            current.workspace.target,
            WorkspaceTarget::Project {
                project_id: project.id.clone()
            }
        );
        assert_eq!(current.workspace.host_cwd, project_path.to_str().unwrap());
        assert_eq!(current.name, "Concurrent project edit");
        let neighbor = client.session("neighbor").await.unwrap().unwrap();
        assert_eq!(neighbor.revision, 1);
        assert_eq!(
            neighbor.workspace.host_cwd,
            directory.path().to_str().unwrap()
        );
        assert!(moved.is_dir(), "switching must not move existing files");
    });
    tui.send(b"\x11");
    tui.finish();
    client.disconnect();
    host.retire_registered();
}
