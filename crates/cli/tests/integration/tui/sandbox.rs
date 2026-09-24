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
fn composer_sandbox_choice_requires_confirmation_and_preserves_approval_and_concurrent_edits() {
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
        client.create_session(decode_session_create_input(&json!({
            "sessionId":"sandbox","name":"Sandbox fixture","workspace":{"kind":"host_path","path":directory.path()},
            "modelTarget":{"kind":"default"},"sandboxMode":"read-only"
        })).unwrap()).await.unwrap();
        client
    });
    let current = || {
        runtime
            .block_on(client.session("sandbox"))
            .unwrap()
            .unwrap()
    };
    let initial = current();
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("Sandbox fixture");
    tui.click_text("Sandbox fixture");
    tui.wait_for("Message…");
    tui.send(b"sandbox keeps draft");
    tui.wait_for("sandbox keeps draft");
    tui.click_last_text("Read only");
    tui.wait_for("Tightening isolation");
    tui.click_text("No sandbox");
    tui.wait_for("Disable sandbox");
    assert_eq!(
        current().revision,
        initial.revision,
        "selecting a mode does not save"
    );
    tui.send(b"\x1b[<0;1;1M\x1b[<0;1;1m");
    tui.wait_until(|screen| !screen.contains("Disable sandbox"));
    assert_eq!(current().sandbox_mode, SandboxMode::ReadOnly);
    for (before, choice, save, mode) in [
        (
            "Read only",
            "Workspace write",
            "Save",
            SandboxMode::WorkspaceWrite,
        ),
        (
            "Workspace write",
            "No sandbox",
            "Disable sandbox",
            SandboxMode::DangerFullAccess,
        ),
        ("No sandbox", "Read only", "Save", SandboxMode::ReadOnly),
    ] {
        tui.wait_for(before);
        tui.click_last_text(before);
        tui.wait_for("Cancel");
        tui.click_text(choice);
        tui.wait_for(save);
        tui.click_text(save);
        tui.wait_until(|screen| !screen.contains("Cancel") && screen.contains(choice));
        assert_eq!(current().sandbox_mode, mode);
        assert_eq!(current().approval_policy, initial.approval_policy);
        assert!(
            tui.screen
                .snapshot()
                .unwrap()
                .screen
                .contains("sandbox keeps draft")
        );
    }
    // Approval categories are independent of isolation. Full bypass is the
    // deliberate compound edit, committed under the same session revision.
    tui.click_last_text("Read only");
    tui.wait_for("Escalation approvals");
    tui.click_text("Escalation approvals");
    tui.wait_for("Ask by category");
    tui.click_text("Ask by category");
    tui.wait_for("Execution outside sandbox");
    tui.click_text("Execution outside sandbox");
    tui.wait_until(|screen| screen.contains("☑ Execution outside sandbox"));
    tui.click_text("Additional permissions");
    tui.wait_until(|screen| screen.contains("☑ Additional permissions"));
    tui.click_text("Save");
    tui.wait_until(|screen| !screen.contains("Cancel"));
    let granular = ApprovalPolicy::Granular {
        sandbox: true,
        rules: false,
        permissions: true,
        client: false,
    };
    assert_eq!(current().sandbox_mode, SandboxMode::ReadOnly);
    assert_eq!(current().approval_policy, granular);
    tui.click_last_text("Read only");
    tui.wait_for("Workspace write");
    tui.click_text("Workspace write");
    tui.wait_for("Save");
    tui.click_text("Save");
    tui.wait_until(|screen| !screen.contains("Cancel") && screen.contains("Workspace write"));
    assert_eq!(current().approval_policy, granular);
    tui.click_last_text("Workspace write");
    tui.wait_for("Full bypass");
    let before_bypass = current();
    tui.click_text("Full bypass");
    tui.wait_for("Enable full bypass");
    assert_eq!(current().revision, before_bypass.revision);
    tui.click_text("Enable full bypass");
    tui.wait_until(|screen| !screen.contains("Cancel") && screen.contains("Full bypass"));
    assert_eq!(current().revision, before_bypass.revision + 1);
    assert_eq!(current().sandbox_mode, SandboxMode::DangerFullAccess);
    assert_eq!(current().approval_policy, ApprovalPolicy::Never);
    tui.click_last_text("Full bypass");
    tui.wait_for("Escalation approvals");
    tui.click_text("Escalation approvals");
    tui.wait_for("Ask when needed");
    tui.click_text("Ask when needed");
    tui.wait_for("Save");
    tui.click_text("Save");
    tui.wait_until(|screen| !screen.contains("Cancel") && screen.contains("No sandbox"));
    assert_eq!(current().approval_policy, ApprovalPolicy::OnRequest);
    assert_eq!(current().sandbox_mode, SandboxMode::DangerFullAccess);
    tui.click_last_text("No sandbox");
    tui.wait_for("Read only");
    tui.click_text("Read only");
    tui.wait_for("Save");
    tui.click_text("Save");
    tui.wait_until(|screen| !screen.contains("Cancel") && screen.contains("Read only"));
    tui.click_last_text("Read only");
    tui.wait_for("Cancel");
    tui.click_text("No sandbox");
    tui.wait_for("Disable sandbox");
    let basis = current();
    runtime
        .block_on(client.update_session_metadata(SessionMetadataUpdateInput {
            session_id: basis.id,
            expected_revision: basis.revision,
            patch: SessionMetadataPatch {
                name: Some("Concurrent title".into()),
                labels: None,
                is_flagged: None,
            },
        }))
        .unwrap();
    tui.click_text("Disable sandbox");
    tui.wait_for("This session changed elsewhere.");
    assert_eq!(current().sandbox_mode, SandboxMode::ReadOnly);
    assert_eq!(current().name, "Concurrent title");
    tui.send(b"\x1b");
    tui.wait_until(|screen| !screen.contains("Cancel"));
    let existing_revision = current().revision;
    let policy = || {
        runtime
            .block_on(client.request(maka_protocol::Operation::RuntimePolicyQuery, json!({})))
            .unwrap()
    };
    assert_eq!(
        policy()["policy"]["chatDefaults"]["sandboxMode"],
        "workspace-write"
    );
    tui.click_text("⛭  Settings");
    tui.wait_for("Sessions");
    tui.click_text("Sessions");
    tui.wait_for("New session sandbox");
    tui.click_text("New session sandbox");
    tui.wait_for("Applies only to future sessions");
    tui.click_text("Read only");
    tui.wait_for("Save");
    tui.click_text("Save");
    tui.wait_until(|screen| !screen.contains("Cancel"));
    assert_eq!(
        policy()["policy"]["chatDefaults"]["sandboxMode"],
        "read-only"
    );
    assert_eq!(
        current().revision,
        existing_revision,
        "defaults never rewrite current sessions"
    );
    // Create through the TUI itself, without an explicit sandbox override.
    tui.wait_until(|screen| {
        screen.contains("Concurrent title") && screen.contains("+  New session")
    });
    tui.click_text("+  New session");
    tui.wait_for("New conversation");
    tui.wait_for("Message…");
    let SessionCatalogQueryResult::Page { sessions, .. } = runtime
        .block_on(client.session_catalog(SessionCatalogQueryInput::ListStart))
        .unwrap()
    else {
        panic!("catalog page")
    };
    let created = sessions
        .iter()
        .find(|session| session.id != "sandbox")
        .unwrap();
    assert_eq!(created.sandbox_mode, SandboxMode::ReadOnly);
    assert_eq!(created.approval_policy, ApprovalPolicy::OnRequest);
    tui.click_text("⛭  Settings");
    tui.wait_for("Sessions");
    tui.click_text("Sessions");
    tui.wait_for("New session sandbox");
    tui.click_text("New session sandbox");
    tui.wait_for("Applies only to future sessions");
    tui.click_text("Workspace write");
    tui.wait_for("Save");
    tui.click_text("Save");
    tui.wait_until(|screen| !screen.contains("Cancel"));
    assert_eq!(
        policy()["policy"]["chatDefaults"]["sandboxMode"],
        "workspace-write"
    );
    assert_eq!(
        runtime
            .block_on(client.session(&created.id))
            .unwrap()
            .unwrap()
            .sandbox_mode,
        SandboxMode::ReadOnly
    );
    tui.click_text("New session sandbox");
    tui.wait_for("Applies only to future sessions");
    tui.click_text("Read only");
    tui.wait_for("Save");
    let basis = policy();
    runtime.block_on(client.request(maka_protocol::Operation::RuntimePolicyMutate, json!({
        "expectedRevision":basis["revision"], "operation":{"kind":"set_workspace_instructions","value":{"enabled":false}}
    }))).unwrap();
    tui.click_text("Save");
    tui.wait_for("Host settings changed elsewhere");
    assert_eq!(
        policy()["policy"]["chatDefaults"]["sandboxMode"],
        "workspace-write"
    );
    assert_eq!(
        policy()["policy"]["workspaceInstructions"]["enabled"],
        false
    );
    tui.send(b"\x1b");
    tui.wait_until(|screen| !screen.contains("Cancel"));
    tui.close_terminal();
    tui.finish();
    client.disconnect();
    host.retire_registered();
    assert!(host.wait_for_exit().success());
}
