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
    event::{EventWrite, Fact, Invocation, InvocationOutcome, RuntimeEvent},
    execution::{PermissionMode, WorkspaceTarget},
    input::InvocationInput,
    workhub::{
        COORDINATION_SESSION_ID, CreateDefaults, CreateModel, CreateSpec, Delegation,
        DelegationDescription, DelegationKind, created_session_id,
    },
};
use serde_json::{Value, json};

#[tokio::test]
async fn workhub_creation_rolls_back_with_its_action_and_replay_preserves_later_session_changes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session(COORDINATION_SESSION_ID, "create", &json!({}), 1)
        .await
        .unwrap();
    let source = Invocation {
        session_id: COORDINATION_SESSION_ID.into(),
        turn_id: "source-turn".into(),
        run_id: "source-run".into(),
        invocation_id: "source-invocation".into(),
    };
    let opening = EventWrite::plain(RuntimeEvent::new(
        source.clone(),
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: "Make a new task".into(),
                request_fingerprint: None,
                source_messages: Vec::new(),
                skill_invocation: None,
            },
        },
    ))
    .unwrap();
    log.append(&opening).await.unwrap();
    let target = created_session_id("create-action");
    let action = EventWrite::plain(RuntimeEvent::new(
        source.clone(),
        Fact::WorkhubDelegated {
            delegation: Box::new(Delegation {
                action_id: "create-action".into(),
                kind: DelegationKind::Created,
                description: Some(DelegationDescription::Created {
                    name: "new task".into(),
                    spec: CreateSpec {
                        title: "  new task  ".into(),
                        workspace: WorkspaceTarget::HostPath {
                            path: "/workspace/.".into(),
                        },
                        defaults: Some(CreateDefaults {
                            permission_mode: Some(PermissionMode::Explore),
                            model: Some(CreateModel {
                                llm_connection_id: "connection".into(),
                                llm_connection_slug: "fixture".into(),
                                model: "fixture-model".into(),
                            }),
                        }),
                    },
                }),
                delivery: Default::default(),
                request_fingerprint: content_digest(b"new request"),
                source_message_event_id: opening.event().id.clone(),
                target: Invocation {
                    session_id: target.clone(),
                    turn_id: "target-turn".into(),
                    run_id: "target-run".into(),
                    invocation_id: "target-invocation".into(),
                },
                target_revision: 1,
                delegation_text: "Execute the task".into(),
            }),
        },
    ))
    .unwrap();
    let config = json!({"name": "new task", "permission_mode": "explore",
        "model": {"connection_id": "connection", "connection_slug": "fixture", "model": "fixture-model"}});
    for (field, value) in [
        ("permission_mode", json!("bypass")),
        (
            "model",
            json!({"connection_id": "other", "connection_slug": "fixture", "model": "fixture-model"}),
        ),
    ] {
        let mut changed = config.clone();
        changed[field] = value;
        assert!(log.create_workhub_session(&action, &changed).await.is_err());
        assert!(log.get_session::<Value>(&target).await.unwrap().is_none());
        assert!(log.workhub_action("create-action").await.unwrap().is_none());
    }
    let before = log
        .list_sessions::<Value>(None, None, 32)
        .await
        .unwrap()
        .revision;
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_workhub_create BEFORE INSERT ON event_log
        WHEN NEW.kind = 'workhub_delegated' BEGIN SELECT RAISE(ABORT, 'injected create failure'); END;").unwrap();
    assert!(log.create_workhub_session(&action, &config).await.is_err());
    assert!(log.get_session::<Value>(&target).await.unwrap().is_none());
    assert!(log.pending_messages(&target).await.unwrap().is_empty());
    assert!(log.workhub_action("create-action").await.unwrap().is_none());
    assert_eq!(
        log.list_sessions::<Value>(None, None, 32)
            .await
            .unwrap()
            .revision,
        before
    );
    db.execute_batch("DROP TRIGGER fail_workhub_create;")
        .unwrap();
    // An unrelated Session at the deterministic identity must not be adopted.
    log.create_session(&target, "foreign", &json!({"name": "foreign"}), 1)
        .await
        .unwrap();
    assert!(log.create_workhub_session(&action, &config).await.is_err());
    assert_eq!(
        log.get_session::<Value>(&target)
            .await
            .unwrap()
            .unwrap()
            .configuration,
        json!({"name": "foreign"})
    );
    db.execute("DELETE FROM session_control WHERE id = ?", [&target])
        .unwrap();
    drop(db);
    let sequence = log.create_workhub_session(&action, &config).await.unwrap();
    assert_eq!(
        log.session_catalog_changes(sequence - 1, sequence, 1)
            .await
            .unwrap(),
        vec![(sequence, target.clone())]
    );
    assert_eq!(
        log.get_session::<Value>(&target)
            .await
            .unwrap()
            .unwrap()
            .configuration,
        config
    );
    assert_eq!(log.pending_messages(&target).await.unwrap().len(), 1);
    log.update_session_metadata(&target, 1, |config: &mut Value| {
        config["name"] = json!("renamed");
        Ok(())
    })
    .await
    .unwrap();
    log.append(
        &EventWrite::plain(RuntimeEvent::new(
            source,
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Completed,
            },
        ))
        .unwrap(),
    )
    .await
    .unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.create_workhub_session(&action, &config).await.unwrap(),
        sequence
    );
    let record = log.get_session::<Value>(&target).await.unwrap().unwrap();
    assert_eq!(record.revision, 2);
    let mut renamed = config.clone();
    renamed["name"] = json!("renamed");
    assert_eq!(record.configuration, renamed);
    assert_eq!(log.pending_messages(&target).await.unwrap().len(), 1);
    log.close().await.unwrap();
}
