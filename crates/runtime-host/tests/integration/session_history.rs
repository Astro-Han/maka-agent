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

use maka_event_log::{EventLog, sessions::SessionCopyResult};
use maka_protocol::session::{RevisionState, SandboxMode, WorkspaceProjection, WorkspaceTarget};
use maka_runtime::{
    event::{EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent},
    input::DeliveredMessage,
    message::{MessageDisposition, Placement, RootSourceMessage},
    session::{CopyPurpose, CopyRequest},
};
use maka_runtime_host::session::{
    PreparedSession, SessionConfiguration, SessionTarget, catalog_projection,
};
use serde_json::json;

#[tokio::test]
async fn catalog_revisions_preserve_branch_origin_without_inheriting_execution_status() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let cwd = directory.path().to_string_lossy().into_owned();
    let configuration = PreparedSession::new(serde_json::from_value(json!({
        "sessionId":"source", "workspace":{"kind":"host_path","path":cwd}, "executorId":"fixture"
    })).unwrap()).unwrap().bind(
        WorkspaceProjection { target: WorkspaceTarget::HostPath { path: cwd.clone() }, host_cwd: cwd },
        SessionTarget::Executor { executor_id: "fixture".to_owned().try_into().unwrap(), settings: Default::default() },
        SandboxMode::ReadOnly,
    );
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("source", "source", &configuration, 1)
        .await
        .unwrap();
    let content: maka_runtime::input::MessageInput = "original".into();
    let invocation = Invocation {
        session_id: "source".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    for fact in [
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                source_messages: vec![RootSourceMessage {
                    unprepared_content: content.clone(),
                    message: DeliveredMessage {
                        message_id: "message".into(),
                        submitted_content_digest: content.content_digest().unwrap(),
                        content: content.clone(),
                    },
                    submitted_placement: Placement::CurrentTurn,
                    disposition: MessageDisposition::TurnStarted,
                    submitted_intent: None,
                }],
                content,
                request_fingerprint: None,
            },
        },
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Failed {
                class: "source_failure".into(),
                message: Some("source failure".into()),
            },
        },
    ] {
        log.append(&EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap())
            .await
            .unwrap();
    }
    for (source, target, purpose) in [
        (
            "source",
            "branch",
            CopyPurpose::Branch {
                turn_id: Some("turn".into()),
                side_conversation: false,
            },
        ),
        (
            "branch",
            "revision",
            CopyPurpose::Revision {
                turn_id: "turn".into(),
            },
        ),
    ] {
        let source = log
            .get_session::<SessionConfiguration>(source)
            .await
            .unwrap()
            .unwrap();
        let result = log
            .copy_session(
                CopyRequest {
                    source_session_id: source.id,
                    target_session_id: target.into(),
                    expected_source_revision: source.revision,
                    purpose,
                },
                &configuration,
                2,
            )
            .await
            .unwrap();
        let SessionCopyResult::Committed(record) = result else {
            panic!("unexpected conflict")
        };
        let projected = catalog_projection(*record);
        maka_protocol::session::decode_session_catalog_projection(
            &serde_json::to_value(&projected).unwrap(),
        )
        .unwrap();
        assert_eq!(projected.parent_session_id.as_deref(), Some("source"));
        assert_eq!(projected.branch_of_turn_id.as_deref(), Some("turn"));
        assert_eq!(
            projected.status,
            maka_protocol::session::SessionStatus::Active
        );
        assert!(projected.live_run_state.is_none());
        if target == "revision" {
            assert_eq!(
                projected.revision_root_session_id.as_deref(),
                Some("branch")
            );
            assert_eq!(
                projected.revision_parent_session_id.as_deref(),
                Some("branch")
            );
            assert_eq!(projected.revision_of_turn_id.as_deref(), Some("turn"));
            assert_eq!(projected.revision_index, Some(2));
            assert_eq!(projected.revision_state, Some(RevisionState::Preparing));
        } else {
            assert!(projected.revision_state.is_none());
        }
    }
    log.retain_session("revision").await.unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    let projection = catalog_projection(
        log.get_session::<SessionConfiguration>("revision")
            .await
            .unwrap()
            .unwrap(),
    );
    assert_eq!(projection.revision_state, Some(RevisionState::Committed));
    assert_eq!(projection.revision_index, Some(2));
    assert!(
        projection.live_run_state.is_none(),
        "retention is not a fabricated execution"
    );
    log.close().await.unwrap();
}
