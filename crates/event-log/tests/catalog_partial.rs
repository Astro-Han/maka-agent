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

use maka_event_log::{EventLog, sessions::SessionExecutionState};
use maka_presentation::{Content, InvocationView};
use maka_runtime::event::EventWrite;
use maka_runtime::{
    event::{
        Fact, Invocation, InvocationInput, InvocationOutcome, LogScope, ModelInterruption,
        RuntimeEvent, StoredEvent,
    },
    model::{ModelEvent, TextKind},
};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::time::{Duration, UNIX_EPOCH};

async fn append(
    log: &EventLog,
    view: &mut InvocationView,
    fact: Fact,
    time: u64,
) -> (u64, Vec<maka_presentation::Row>) {
    let mut event = RuntimeEvent::new(
        Invocation {
            session_id: "a".into(),
            turn_id: "turn".into(),
            run_id: "run".into(),
            invocation_id: "invocation".into(),
        },
        fact,
    );
    event.recorded_at = UNIX_EPOCH + Duration::from_millis(time);
    let sequence = log
        .append(&EventWrite::plain((event).clone()).unwrap())
        .await
        .unwrap();
    (
        sequence,
        view.push(&StoredEvent { sequence, event }).unwrap(),
    )
}
fn observe(event: ModelEvent) -> Fact {
    Fact::ModelObserved {
        step_id: "step".into(),
        event,
    }
}
fn source(db: &Connection) -> Vec<String> {
    db.prepare("SELECT event_json FROM runtime_events ORDER BY sequence")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn interrupted_and_terminal_fallback_catalog_match_committed_presentation_and_rebuild() {
    for (interrupt, outcome) in [
        (
            true,
            InvocationOutcome::Cancelled {
                source: "user".into(),
            },
        ),
        (
            false,
            InvocationOutcome::Failed {
                class: "event_commit".into(),
                message: None,
            },
        ),
        (
            false,
            InvocationOutcome::Cancelled {
                source: "shutdown".into(),
            },
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.sqlite");
        let log = EventLog::open(&path).await.unwrap();
        log.create_session("a", "request", &json!({}), 1)
            .await
            .unwrap();
        let mut view = InvocationView::new(16 * 1024 * 1024).unwrap();
        append(
            &log,
            &mut view,
            Fact::InvocationOpened {
                configuration: None,
                input: InvocationInput::Message {
                    source_messages: Vec::new(),
                    content: "user".into(),
                    request_fingerprint: None,
                },
            },
            1000,
        )
        .await;
        append(
            &log,
            &mut view,
            Fact::ModelRequested {
                purpose: maka_runtime::context::ModelPurpose::Main,
                context: None,
                checkpoint_event_id: None,
                step_id: "step".into(),
                model_id: "real".into(),
                source_scope: LogScope::Root,
                source_high_water: 0,
                source_digest: "source".into(),
                effective_source_digest: None,
                input_digest: "input".into(),
                route_identity: "route".into(),
            },
            1100,
        )
        .await;
        append(
            &log,
            &mut view,
            observe(ModelEvent::PartStarted {
                id: "part".into(),
                text_kind: TextKind::Text,
                provider_options: None,
            }),
            2000,
        )
        .await;
        let mut live = 0;
        for chunk in [" \n par", "tial\t", " \n", "😀  tail "] {
            live = append(
                &log,
                &mut view,
                observe(ModelEvent::PartDelta {
                    id: "part".into(),
                    text: chunk.into(),
                    provider_options: None,
                }),
                2100,
            )
            .await
            .0;
        }
        // Deltas alone do not advertise a sealed catalog message.
        let before = log.get_session::<Value>("a").await.unwrap().unwrap();
        assert_eq!(
            before
                .execution
                .as_ref()
                .unwrap()
                .last_message
                .as_ref()
                .unwrap()
                .recorded_at,
            1000
        );
        let mut assistant = None;
        if interrupt {
            // The preview becomes full before a giant later delta. The boundary
            // index reads only enough committed text for the normalized prefix.
            for chunk in ["答".repeat(120), "x".repeat(9 * 1024 * 1024)] {
                append(
                    &log,
                    &mut view,
                    observe(ModelEvent::PartDelta {
                        id: "part".into(),
                        text: chunk,
                        provider_options: None,
                    }),
                    2150,
                )
                .await;
            }
            let (boundary, rows) = append(
                &log,
                &mut view,
                Fact::ModelInterrupted {
                    step_id: "step".into(),
                    status: ModelInterruption::Cancelled,
                },
                3000,
            )
            .await;
            assistant = rows
                .into_iter()
                .find(|row| matches!(row.message.content, Content::Assistant { .. }));
            let interrupted = log.get_session::<Value>("a").await.unwrap().unwrap();
            assert_eq!(interrupted.revision, before.revision + 1);
            assert_eq!(
                interrupted.execution.as_ref().unwrap().state,
                SessionExecutionState::Live { recorded_at: 1000 }
            );
            assert_eq!(
                log.session_catalog_changes(live, boundary, 129)
                    .await
                    .unwrap(),
                [(boundary, "a".into())]
            );
            assert!(
                log.append(
                    &EventWrite::plain(
                        (RuntimeEvent::new(
                            Invocation {
                                session_id: "a".into(),
                                turn_id: "turn".into(),
                                run_id: "run".into(),
                                invocation_id: "invocation".into(),
                            },
                            observe(ModelEvent::PartDelta {
                                id: "part".into(),
                                text: "uncommitted".into(),
                                provider_options: None
                            })
                        ))
                        .clone()
                    )
                    .unwrap()
                )
                .await
                .is_err()
            );
        }
        let (terminal, rows) =
            append(&log, &mut view, Fact::InvocationEnded { outcome }, 900).await;
        if !interrupt {
            assistant = rows
                .into_iter()
                .find(|row| matches!(row.message.content, Content::Assistant { .. }));
            assert_eq!(
                log.session_catalog_changes(live, terminal, 129)
                    .await
                    .unwrap(),
                [(terminal, "a".into())]
            );
        }
        let assistant = assistant.unwrap().message;
        let Content::Assistant { text, .. } = assistant.content else {
            unreachable!()
        };
        let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let expected = if normalized.chars().count() > 96 {
            format!("{}…", normalized.chars().take(95).collect::<String>())
        } else {
            normalized
        };
        let ended = log.get_session::<Value>("a").await.unwrap().unwrap();
        let message = ended
            .execution
            .as_ref()
            .unwrap()
            .last_message
            .as_ref()
            .unwrap();
        assert_eq!(message.recorded_at, assistant.ts);
        assert_eq!(message.recorded_at, 2000); // Not interruption@3000 or terminal@900.
        assert_eq!(message.preview.as_deref(), Some(expected.as_str()));
        assert_eq!(ended.revision, if interrupt { 4 } else { 3 });
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM runtime_events WHERE kind = 'model_completed'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        let original = source(&db);
        log.close().await.unwrap();
        let reopened = EventLog::open(&path).await.unwrap();
        assert_eq!(
            reopened.get_session::<Value>("a").await.unwrap().unwrap(),
            ended
        );
        reopened.close().await.unwrap();
        db.execute_batch("DELETE FROM catalog_messages;").unwrap();
        let rebuilt = EventLog::open(&path).await.unwrap();
        assert_eq!(
            rebuilt.get_session::<Value>("a").await.unwrap().unwrap(),
            ended
        );
        assert_eq!(source(&db), original);
    }
}
