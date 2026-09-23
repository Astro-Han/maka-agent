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

use maka_event_log::{
    EventLog, StoreError,
    sessions::{ImportState, PluginSession, SessionCopy, SessionCopyResult},
};
use maka_plugins::{composition::Scope, storage::Namespace};
use maka_runtime::{
    event::{
        EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, LogScope, RuntimeEvent,
    },
    import::{Content, Record, Source},
};
use serde_json::{Value, json};

fn owner(id: &str) -> PluginSession {
    PluginSession {
        session_id: id.into(),
        creator: Namespace::new("external.reader", Scope::Profile).unwrap(),
        fingerprint: format!("request-{id}"),
        managed: false,
        authority_session_id: None,
    }
}
fn source() -> Source {
    Source {
        adapter: "arbitrary-format".into(),
        session_id: "foreign-session".into(),
    }
}
fn record(id: &str, content: Content) -> Record {
    Record {
        source_message_id: id.into(),
        source_turn_id: "foreign-turn".into(),
        timestamp: Some(123),
        content,
    }
}

#[tokio::test]
async fn import_is_retryable_history_not_an_execution_and_survives_copy_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("import.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let owner = owner("imported");
    let first = record(
        "leading",
        Content::Assistant {
            text: "retained, not a prefill".into(),
            model: None,
            thinking: None,
        },
    );
    let records = vec![
        record(
            "user",
            Content::User {
                text: "the original question".into(),
            },
        ),
        record(
            "tool",
            Content::ToolCall {
                call_id: "foreign-call".into(),
                name: "old-command".into(),
                input: Some(json!({"command":"do not run"})),
            },
        ),
        record(
            "answer",
            Content::Assistant {
                text: "the original answer".into(),
                model: Some("foreign-model".into()),
                thinking: Some("original thought".into()),
            },
        ),
        record(
            "result",
            Content::ToolResult {
                call_id: "foreign-call".into(),
                output: json!({"result":"historical"}),
                is_error: false,
            },
        ),
    ];
    log.begin_session_import(&owner, &source(), &json!({"name":"Imported"}), 1)
        .await
        .unwrap();
    log.append_session_import("imported", 0, vec![first.clone()])
        .await
        .unwrap();
    assert!(
        log.get_session::<Value>("imported")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        log.create_session("imported", &owner.fingerprint, &json!({}), 1)
            .await
            .is_err()
    );
    assert!(log.retain_session("imported").await.is_err());
    assert!(
        log.append_session_import("imported", 2, records.clone())
            .await
            .is_err()
    );
    let prefix = log
        .scoped_prefix(
            LogScope::Session {
                id: "imported".into(),
            },
            100,
            1024 * 1024,
        )
        .await
        .unwrap();
    assert!(EventWrite::plain(prefix.events[0].event.clone()).is_err());
    assert!(
        maka_agent::project_model_history(&prefix, "imported")
            .unwrap()
            .is_empty()
    );
    assert!(log.unfinished_invocations(100).await.unwrap().is_empty());
    log.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.begin_session_import(&owner, &source(), &json!({"name":"Imported"}), 99)
            .await
            .unwrap()
            .records,
        1
    );
    assert_eq!(
        log.append_session_import("imported", 0, vec![first.clone()])
            .await
            .unwrap()
            .records,
        1
    );
    let changed = record(
        "leading",
        Content::User {
            text: "changed".into(),
        },
    );
    assert!(matches!(
        log.append_session_import("imported", 0, vec![changed])
            .await,
        Err(StoreError::EventConflict)
    ));
    log.append_session_import("imported", 1, records)
        .await
        .unwrap();
    assert!(log.publish_session_import("imported", 3).await.is_err());
    let receipt = log.publish_session_import("imported", 5).await.unwrap();
    assert_eq!(receipt.state, ImportState::Published);
    assert_eq!(
        log.publish_session_import("imported", 5).await.unwrap(),
        receipt
    );
    assert_eq!(
        log.abandon_session_import("imported").await.unwrap(),
        receipt
    );
    let session = log.get_session::<Value>("imported").await.unwrap().unwrap();
    assert!(session.execution.is_none());
    assert_eq!(
        session.last_message.as_ref().unwrap().preview.as_deref(),
        Some("the original answer")
    );
    let source = log
        .read_model_context("imported", None, 100, 1024 * 1024)
        .await
        .unwrap();
    let frozen = source.source_evidence;
    let prefix = log
        .scoped_prefix(
            LogScope::Session {
                id: "imported".into(),
            },
            100,
            1024 * 1024,
        )
        .await
        .unwrap();
    let history = maka_agent::project_model_history(&prefix, "imported").unwrap();
    assert_eq!(history.len(), 2);
    assert!(
        serde_json::to_string(&history)
            .unwrap()
            .contains("the original answer")
    );
    assert!(
        !serde_json::to_string(&history)
            .unwrap()
            .contains("do not run")
    );
    assert!(
        !serde_json::to_string(&history)
            .unwrap()
            .contains("original thought")
    );
    while !log
        .prepare_transcript("imported", prefix.high_water, 32)
        .await
        .unwrap()
    {}
    let read = log
        .history_text("imported", prefix.high_water, None)
        .await
        .unwrap();
    let text = serde_json::to_string(&read).unwrap();
    assert!(text.contains("retained, not a prefill"));
    assert!(text.contains("the original answer"));

    let rows: Vec<_> = prefix
        .events
        .iter()
        .flat_map(|event| {
            maka_presentation::InvocationView::new(1024 * 1024)
                .unwrap()
                .push(event)
                .unwrap()
        })
        .collect();
    assert_eq!(rows.len(), 5);
    let call = &rows[2].message;
    assert!(matches!(
        rows[3].message.content,
        maka_presentation::Content::Assistant { .. }
    ));
    let maka_presentation::Content::ToolResult { tool_use_id, .. } = &rows[4].message.content
    else {
        panic!("result must retain its position after the intervening answer");
    };
    assert_eq!(tool_use_id, &call.id);

    let copy = log
        .copy_session(
            SessionCopy {
                source_session_id: "imported".into(),
                target_session_id: "copy".into(),
                expected_source_revision: session.revision,
                purpose: maka_runtime::session::CopyPurpose::Branch {
                    turn_id: None,
                    side_conversation: false,
                },
            },
            &json!({}),
            2,
        )
        .await
        .unwrap();
    assert!(matches!(copy, SessionCopyResult::Committed(_)));
    assert_eq!(
        log.read_model_context("copy", None, 100, 1024 * 1024)
            .await
            .unwrap()
            .tail
            .len(),
        5
    );

    let invocation = Invocation {
        session_id: "imported".into(),
        turn_id: "new-turn".into(),
        run_id: "new-run".into(),
        invocation_id: "new-invocation".into(),
    };
    for fact in [
        Fact::InvocationOpened {
            input: InvocationInput::Message {
                content: "continue".into(),
                request_fingerprint: None,
                source_messages: vec![],
            },
            configuration: None,
        },
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    ] {
        log.append(&EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap())
            .await
            .unwrap();
    }
    assert_eq!(
        log.read_frozen_model_context(&frozen, 100, 1024 * 1024)
            .await
            .unwrap()
            .tail
            .len(),
        5
    );
    let prefix = log
        .scoped_prefix(
            LogScope::Session {
                id: "imported".into(),
            },
            100,
            1024 * 1024,
        )
        .await
        .unwrap();
    assert_eq!(
        maka_agent::project_model_history(&prefix, "imported")
            .unwrap()
            .len(),
        3
    );
    assert!(!prefix.events.iter().any(|event| matches!(
        event.event.fact,
        Fact::ModelRequested { .. } | Fact::ToolDispatched { .. }
    )));
    log.close().await.unwrap();
}

#[tokio::test]
async fn nonconversation_import_cannot_publish_and_abandon_releases_material() {
    let directory = tempfile::tempdir().unwrap();
    let log = EventLog::open(&directory.path().join("abandon.sqlite"))
        .await
        .unwrap();
    log.create_session("parent", "parent", &json!({}), 1)
        .await
        .unwrap();
    let mut child = owner("dependent");
    child.authority_session_id = Some("parent".into());
    log.begin_session_import(&child, &source(), &json!({}), 1)
        .await
        .unwrap();
    log.append_session_import(
        "dependent",
        0,
        vec![record(
            "user",
            Content::User {
                text: "historical".into(),
            },
        )],
    )
    .await
    .unwrap();
    let parent = log.get_session::<Value>("parent").await.unwrap().unwrap();
    log.begin_session_removal("parent", parent.revision)
        .await
        .unwrap();
    assert!(log.publish_session_import("dependent", 1).await.is_err());
    assert_eq!(
        log.session_import_progress("dependent")
            .await
            .unwrap()
            .state,
        ImportState::Collecting
    );
    log.abandon_session_import("dependent").await.unwrap();
    log.begin_session_import(&owner("notes"), &source(), &json!({}), 1)
        .await
        .unwrap();
    assert!(
        log.append_session_import(
            "notes",
            0,
            vec![record(
                "oversize",
                Content::User {
                    text: "x".repeat(maka_runtime::import::MAX_IMPORT_BYTES as usize),
                }
            )]
        )
        .await
        .is_err()
    );
    assert_eq!(
        log.session_import_progress("notes").await.unwrap().records,
        0
    );
    log.append_session_import(
        "notes",
        0,
        vec![record(
            "note",
            Content::Note {
                text: "not a conversation".into(),
            },
        )],
    )
    .await
    .unwrap();
    assert!(log.publish_session_import("notes", 1).await.is_err());
    assert_eq!(
        log.abandon_session_import("notes").await.unwrap().state,
        ImportState::Abandoned
    );
    assert!(log.publish_session_import("notes", 1).await.is_err());
    while log.collect_session_material(None).await.unwrap()
        != maka_event_log::sessions::MaterialCollection::Done
    {}
    assert!(matches!(
        log.scoped_prefix(LogScope::Session { id: "notes".into() }, 100, 1024 * 1024)
            .await,
        Err(StoreError::MaterialCollected)
    ));
    assert!(log.get_session::<Value>("notes").await.unwrap().is_none());
    log.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imported_conversation_compacts_before_its_first_native_turn() {
    use crate::support::context::{SUMMARY, engine, input, read_request, respond};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    let directory = tempfile::tempdir().unwrap();
    let log = Arc::new(
        EventLog::open(&directory.path().join("compact.sqlite"))
            .await
            .unwrap(),
    );
    log.begin_session_import(&owner("session"), &source(), &json!({}), 1)
        .await
        .unwrap();
    log.append_session_import(
        "session",
        0,
        vec![
            record(
                "user",
                Content::User {
                    text: "original-import-question".into(),
                },
            ),
            record("blank", Content::User { text: "  ".into() }),
            record(
                "thought",
                Content::Assistant {
                    text: String::new(),
                    model: None,
                    thinking: Some("historical reasoning".into()),
                },
            ),
            record(
                "answer",
                Content::Assistant {
                    text: "original-import-answer".into(),
                    model: None,
                    thinking: None,
                },
            ),
        ],
    )
    .await
    .unwrap();
    log.publish_session_import("session", 4).await.unwrap();
    let prefix = log
        .scoped_prefix(
            LogScope::Session {
                id: "session".into(),
            },
            100,
            128 * 1024,
        )
        .await
        .unwrap();
    assert_eq!(
        maka_agent::project_model_history(&prefix, "session")
            .unwrap()
            .len(),
        2
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for text in [SUMMARY, "continued-answer"] {
            let (mut socket, _) =
                tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            requests.push(read_request(&mut socket).await);
            respond(&mut socket, text, "stop").await;
        }
        requests
    });
    let worker = engine(log.clone());
    worker
        .run(input(&base, "compact", true), CancellationToken::new())
        .await
        .unwrap();
    assert!(
        log.read_model_context("session", None, 100, 128 * 1024)
            .await
            .unwrap()
            .baseline
            .is_some()
    );
    worker
        .run(input(&base, "continue", false), CancellationToken::new())
        .await
        .unwrap();
    worker.drain().await;
    let requests = server.await.unwrap();
    assert!(requests[0].to_string().contains("original-import-answer"));
    assert!(!requests[1].to_string().contains("original-import-answer"));
    assert!(requests[1].to_string().contains("summary-marker"));
    drop(worker);
    Arc::try_unwrap(log).ok().unwrap().close().await.unwrap();
}
