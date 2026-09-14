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

use maka_event_log::EventLog;
use maka_runtime::{
    artifact::content_digest,
    event::{CommitError, EventWrite, Fact, Invocation, RuntimeEvent},
    input::InvocationInput,
    workhub::{COORDINATION_SESSION_ID, Delegation},
};
use serde_json::{Value, json};

#[tokio::test]
async fn target_metadata_change_invalidates_uncommitted_delegation_but_not_its_durable_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let log = EventLog::open(&temp.path().join("events.sqlite"))
        .await
        .unwrap();
    for session in [COORDINATION_SESSION_ID, "target"] {
        log.create_session(session, "create", &json!({"name": "original"}), 1)
            .await
            .unwrap();
    }
    let coordinator = Invocation {
        session_id: COORDINATION_SESSION_ID.into(),
        turn_id: "source-turn".into(),
        run_id: "source-run".into(),
        invocation_id: "source-invocation".into(),
    };
    let source = EventWrite::plain(RuntimeEvent::new(
        coordinator.clone(),
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: "user request".into(),
                request_fingerprint: None,
                source_messages: Vec::new(),
                skill_invocation: None,
            },
        },
    ))
    .unwrap();
    log.append(&source).await.unwrap();
    let boundary = log
        .turn_boundary(COORDINATION_SESSION_ID, "source-turn")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(boundary.opening_event_id, source.event().id);
    let mut delegation = Delegation {
        action_id: "action".into(),
        request_fingerprint: content_digest(b"request"),
        source_message_event_id: boundary.opening_event_id,
        target: Invocation {
            session_id: "target".into(),
            turn_id: "target-turn".into(),
            run_id: "target-run".into(),
            invocation_id: "target-invocation".into(),
        },
        target_revision: 1,
        delegation_text: "delegated task".into(),
    };
    log.update_session_metadata("target", 1, |config: &mut Value| {
        config["name"] = json!("changed after candidate selection");
        Ok(())
    })
    .await
    .unwrap();
    let write = |delegation| {
        EventWrite::plain(RuntimeEvent::new(
            coordinator.clone(),
            Fact::WorkhubDelegated {
                delegation: Box::new(delegation),
            },
        ))
        .unwrap()
    };
    let rejected = log.append(&write(delegation.clone())).await;
    assert!(
        matches!(&rejected, Err(CommitError::Rejected(reason)) if reason.contains("revision")),
        "{rejected:?}"
    );
    assert!(log.pending_messages("target").await.unwrap().is_empty());
    assert!(log.workhub_action("action").await.unwrap().is_none());
    delegation.target_revision = 2;
    let action = write(delegation);
    let receipt = log.append(&action).await.unwrap();
    log.update_session_metadata("target", 2, |config: &mut Value| {
        config["name"] = json!("changed after commit");
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(log.append(&action).await.unwrap(), receipt);
    assert_eq!(log.pending_messages("target").await.unwrap().len(), 1);
    log.close().await.unwrap();
}
