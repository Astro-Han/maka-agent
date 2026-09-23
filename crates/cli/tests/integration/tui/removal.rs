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
fn deletion_confirms_cas_and_remote_retirement_preserves_recoverable_draft() {
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
        for (id, name) in [
            ("local-delete", "Local deletion"),
            ("remote-delete", "Remote deletion"),
            ("neighbor", "Untouched neighbor"),
        ] {
            client
                .create_session(
                    decode_session_create_input(&json!({
                        "sessionId": id, "name": name,
                        "workspace": {"kind":"host_path", "path":directory.path()},
                        "modelTarget": {"kind":"default"}
                    }))
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        client
    });
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Local deletion");
    tui.click_text("Local deletion");
    tui.wait_for("Message…");
    tui.send(b"\x10");
    tui.wait_for("Delete session");
    tui.click_text("Delete session");
    tui.wait_for("Permanently deletes");
    tui.send(b"\r"); // Cancel is the initial focus.
    tui.wait_until(|s| !s.contains("Permanently deletes"));
    assert!(
        runtime
            .block_on(client.session("local-delete"))
            .unwrap()
            .is_some()
    );
    tui.send(b"\x10");
    tui.wait_for("Delete session");
    tui.click_text("Delete session");
    tui.wait_for("Permanently deletes");
    runtime.block_on(async {
        let current = client.session("local-delete").await.unwrap().unwrap();
        client
            .update_session_metadata(SessionMetadataUpdateInput {
                session_id: current.id,
                expected_revision: current.revision,
                patch: SessionMetadataPatch {
                    name: Some("Concurrent deletion target".into()),
                    labels: None,
                    is_flagged: None,
                },
            })
            .await
            .unwrap();
    });
    tui.send(b"\t\r");
    tui.wait_for("This session changed elsewhere.");
    assert!(
        runtime
            .block_on(client.session("local-delete"))
            .unwrap()
            .is_some()
    );
    tui.send(b"\x1b");
    tui.wait_until(|s| {
        s.contains("Concurrent deletion target") && !s.contains("This session changed elsewhere.")
    });
    tui.send(b"\x10");
    tui.wait_for("Delete session");
    tui.click_text("Delete session");
    tui.wait_for("Permanently deletes");
    tui.click_last_text("Delete");
    tui.wait_until(|s| {
        s.contains("Remote deletion")
            && !s.contains("Concurrent deletion target")
            && !s.contains("Permanently deletes")
    });
    assert!(
        runtime
            .block_on(client.session("local-delete"))
            .unwrap()
            .is_none()
    );
    assert!(
        directory.path().is_dir(),
        "ordinary Host workspace is not deleted"
    );
    tui.click_text("Remote deletion");
    tui.wait_for("Message…");
    tui.send(b"recover this local draft");
    tui.wait_for("recover this local draft");
    tui.send(b"\x10");
    tui.wait_for("Delete session");
    tui.click_text("Delete session");
    tui.wait_for("Permanently deletes");
    runtime.block_on(async {
        let current = client.session("remote-delete").await.unwrap().unwrap();
        assert!(matches!(
            client
                .remove_session(SessionRemoveInput {
                    session_id: current.id,
                    expected_revision: current.revision,
                })
                .await
                .unwrap(),
            SessionRemoveResult::Removed { .. }
        ));
    });
    tui.wait_until(|s| {
        s.contains("This session is no longer available.") && !s.contains("Permanently deletes")
    });
    assert!(
        tui.screen
            .snapshot()
            .unwrap()
            .screen
            .contains("recover this local draft")
    );
    tui.send(b"\x11");
    tui.finish();
    let mut reopened = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    reopened.wait_for("This session is no longer available.");
    assert!(
        reopened
            .screen
            .snapshot()
            .unwrap()
            .screen
            .contains("recover this local draft")
    );
    assert_eq!(
        runtime
            .block_on(client.session("neighbor"))
            .unwrap()
            .unwrap()
            .revision,
        1
    );
    reopened.send(b"\x11");
    reopened.finish();
    client.disconnect();
    host.retire_registered();
}
