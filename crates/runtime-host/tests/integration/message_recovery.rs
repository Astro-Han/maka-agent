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
    message_recovery::{Provider, configure, unknown_dispatch},
};
use maka_event_log::{EventLog, message_admissions::PendingMessageAdmission};
use maka_protocol::session::SandboxMode;
use maka_runtime::{
    event::{EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent},
    input::DeliveredMessage,
    message::{MessageDisposition as Disposition, Placement, RootSourceMessage},
};
use maka_runtime_host::session::PreparedSession;
use serde_json::json;

fn invocation(session: &str) -> Invocation {
    Invocation {
        session_id: session.into(),
        turn_id: format!("reserved-{session}"),
        run_id: format!("run-{session}"),
        invocation_id: format!("invocation-{session}"),
    }
}
async fn append(log: &EventLog, owner: &Invocation, fact: Fact) {
    log.append(&EventWrite::plain(RuntimeEvent::new(owner.clone(), fact)).unwrap())
        .await
        .unwrap();
}
async fn pending(log: &EventLog, owner: &Invocation, id: &str, disposition: Disposition) {
    let content: maka_runtime::input::MessageInput = id.into();
    log.admit_message(PendingMessageAdmission {
        invocation: owner.clone(),
        steering_invocation: None,
        required_tools: if id == "skill-gate" {
            ["NeverRegisteredFixtureTool".into()].into()
        } else {
            Default::default()
        },
        admitted_at: 2,
        source: RootSourceMessage {
            message: DeliveredMessage {
                message_id: id.into(),
                submitted_content_digest: content.content_digest().unwrap(),
                content,
            },
            submitted_placement: if disposition == Disposition::Followup {
                Placement::NextTurn
            } else {
                Placement::CurrentTurn
            },
            disposition,
            submitted_intent: None,
        },
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_consumes_reserved_and_queued_work_once_without_replaying_unknown_effects() {
    let fixture = ClientFixture::new("maka-message-recovery-");
    let provider = Provider::start().await;
    let model = configure(&fixture, &provider.base_url).await;
    let log = fixture.log().await;
    for session in ["reserved", "queued", "missing", "unknown", "skill-gate"] {
        let mut model = model.clone();
        if session == "missing" {
            model.connection_id = "removed-provider".into();
        }
        let configuration = PreparedSession::new(
            serde_json::from_value(json!({
                "sessionId":session, "workspace":{"kind":"host_path","path":fixture.workspace},
                "modelTarget":{"kind":"default"}
            }))
            .unwrap(),
        )
        .unwrap()
        .bind(
            maka_protocol::session::WorkspaceProjection {
                target: maka_protocol::session::WorkspaceTarget::HostPath {
                    path: fixture.workspace.to_string_lossy().into_owned(),
                },
                host_cwd: fixture.workspace.to_string_lossy().into_owned(),
            },
            model,
            SandboxMode::ReadOnly,
            maka_runtime::execution::ToolMode::Direct,
        );
        log.create_session(session, "fixture", &configuration, 1)
            .await
            .unwrap();
        let owner = invocation(session);
        if matches!(session, "reserved" | "missing" | "skill-gate") {
            pending(&log, &owner, session, Disposition::TurnStarted).await;
        } else {
            append(
                &log,
                &owner,
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        content: "previous root".into(),
                        request_fingerprint: None,
                        source_messages: vec![],
                    },
                },
            )
            .await;
            pending(
                &log,
                &owner,
                &format!("{session}-followup"),
                Disposition::Followup,
            )
            .await;
            if session == "queued" {
                pending(&log, &owner, "queued-steering", Disposition::Steering).await;
                append(
                    &log,
                    &owner,
                    Fact::InvocationEnded {
                        outcome: InvocationOutcome::Completed,
                    },
                )
                .await;
            } else {
                unknown_dispatch(&log, &owner, &fixture.workspace.join("must-not-replay")).await;
            }
        }
    }
    let before = log.prefix(100, 1024 * 1024).await.unwrap();
    log.close().await.unwrap();
    fixture
        .run(
            "--message-recovery-workspace",
            false,
            "message-recovery-passed",
        )
        .await;
    let log = fixture.log().await;
    let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
    assert_eq!(
        serde_json::to_value(&prefix.events[..before.events.len()]).unwrap(),
        serde_json::to_value(&before.events).unwrap()
    );
    for session in ["reserved", "queued", "missing", "unknown", "skill-gate"] {
        assert!(log.pending_messages(session).await.unwrap().is_empty());
    }
    for session in ["reserved", "missing", "skill-gate"] {
        assert_eq!(
            log.root_message(session, session)
                .await
                .unwrap()
                .unwrap()
                .opening()
                .event
                .invocation,
            invocation(session)
        );
    }
    let steering = log
        .root_message("queued", "queued-steering")
        .await
        .unwrap()
        .unwrap();
    let followup = log
        .root_message("queued", "queued-followup")
        .await
        .unwrap()
        .unwrap();
    assert!(steering.opening().sequence < followup.opening().sequence);
    assert_ne!(
        steering.opening().event.invocation.turn_id,
        invocation("queued").turn_id
    );
    assert_ne!(
        steering.opening().event.invocation.turn_id,
        followup.opening().event.invocation.turn_id
    );
    assert!(!fixture.workspace.join("must-not-replay").exists());
    assert_eq!(
        prefix
            .events
            .iter()
            .filter(|e| matches!(e.event.fact, Fact::ToolDispatched { .. }))
            .count(),
        1
    );
    assert!(
        !prefix
            .events
            .iter()
            .any(|e| matches!(e.event.fact, Fact::ToolSettled { .. }))
    );
    let requests = provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    let queued: Vec<_> = requests
        .iter()
        .filter_map(|body| {
            body["messages"]
                .as_array()?
                .iter()
                .rev()
                .find(|m| m["role"] == "user")?["content"]
                .as_str()
        })
        .filter(|text| text.starts_with("queued-"))
        .collect();
    assert_eq!(queued, ["queued-steering", "queued-followup"]);
    let prefix = serde_json::to_vec(&prefix).unwrap();
    log.close().await.unwrap();
    fixture
        .run(
            "--message-recovery-workspace",
            true,
            "message-recovery-reopened",
        )
        .await;
    let log = fixture.log().await;
    assert_eq!(
        serde_json::to_vec(&log.prefix(100, 1024 * 1024).await.unwrap()).unwrap(),
        prefix
    );
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
    log.close().await.unwrap();
}
