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
    EventLog, observation::StreamFact, sessions::SessionExecutionState, turns::InvocationState,
};
use maka_presentation::InvocationView;
use maka_runtime::{
    context::{CheckpointMode, CompactOutcome, ModelPurpose},
    event::{
        EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, LogScope,
        ModelInterruption, RuntimeEvent, StoredEvent,
    },
    model::{ModelEvent, ModelFinishReason, ModelPart, ModelStep, TextKind},
};
use rusqlite::Connection;
use serde_json::{Value, json};

#[path = "compact_presentation/fixtures.rs"]
mod fixtures;
use fixtures::*;
#[path = "compact_presentation/automatic.rs"]
mod automatic;

#[tokio::test]
async fn compact_attempts_preserve_visible_history_read_markers_and_rebuild() {
    for mode in ["accepted", "interrupted", "failed"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.sqlite");
        let log = EventLog::open(&path).await.unwrap();
        log.create_session("session", "create", &json!({}), 1)
            .await
            .unwrap();
        let ordinary = identity("ordinary");
        let opened = append(
            &log,
            &ordinary,
            Fact::InvocationOpened {
                configuration: None,
                input: InvocationInput::Message {
                    source_messages: Vec::new(),
                    content: "old visible message".into(),
                    request_fingerprint: None,
                },
            },
        )
        .await;
        let old_end = append(
            &log,
            &ordinary,
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Completed,
            },
        )
        .await;
        let previous = log
            .set_session_read_marker::<Value>("session", &opened.event.id)
            .await
            .unwrap();
        assert!(!previous.read_state.has_unread);
        assert!(
            log.prepare_transcript("session", old_end.sequence, 32)
                .await
                .unwrap()
        );
        let db = Connection::open(&path).unwrap();
        let old_rows = transcript(&db);
        let compact = identity(mode);
        let mut view = InvocationView::new(1).unwrap();
        hidden(
            &log,
            &mut view,
            &compact,
            Fact::InvocationOpened {
                configuration: None,
                input: InvocationInput::ContextCompact {
                    request_fingerprint: format!("compact-{mode}"),
                },
            },
        )
        .await;
        let source = log
            .prepare_context_compaction(
                "session",
                Some(&compact.invocation_id),
                100,
                1024 * 1024,
                &CheckpointMode::Standalone,
            )
            .await
            .unwrap();
        hidden(
            &log,
            &mut view,
            &compact,
            Fact::ModelRequested {
                purpose: ModelPurpose::Summary,
                context: None,
                step_id: "summary".into(),
                model_id: "model".into(),
                source_scope: source.source_evidence.scope,
                source_high_water: source.source_evidence.high_water,
                source_digest: source.source_evidence.digest,
                effective_source_digest: Some(source.effective_source_digest),
                input_digest: "input".into(),
                route_identity: "route".into(),
                checkpoint_event_id: None,
            },
        )
        .await;
        hidden(
            &log,
            &mut view,
            &compact,
            observation(ModelEvent::PartStarted {
                id: "part".into(),
                text_kind: TextKind::Text,
                provider_options: None,
            }),
        )
        .await;
        let secret = "private summary text ".repeat(100);
        let delta = hidden(
            &log,
            &mut view,
            &compact,
            observation(ModelEvent::PartDelta {
                id: "part".into(),
                text: secret.clone(),
                provider_options: None,
            }),
        )
        .await;
        let running = log
            .observe_session::<Value>("session")
            .await
            .unwrap()
            .unwrap();
        let root = running.root_turn.unwrap();
        assert_eq!(root.invocation, compact);
        assert!(matches!(root.input, InvocationInput::ContextCompact { .. }));
        assert!(matches!(root.state, InvocationState::Running));
        assert!(running.active_streams.is_empty());
        assert_eq!(running.session.read_state, previous.read_state);
        assert_eq!(
            running.session.execution.unwrap().last_message,
            previous.execution.as_ref().unwrap().last_message
        );
        assert!(
            log.active_transcript(&compact, delta.sequence)
                .await
                .unwrap()
                .is_empty()
        );
        let delivery = log
            .session_stream_events("session", old_end.sequence, delta.sequence, 1, 1)
            .await
            .unwrap();
        assert!(delivery.events.is_empty());
        assert!(delivery.next_after.is_none());
        assert_eq!(delivery.through_sequence, delta.sequence);
        assert!(
            log.prepare_transcript("session", delta.sequence, 1)
                .await
                .unwrap()
        );
        assert_eq!(transcript(&db), old_rows);
        let outcome = match mode {
            "accepted" => {
                hidden(
                    &log,
                    &mut view,
                    &compact,
                    observation(ModelEvent::PartFinished {
                        id: "part".into(),
                        provider_options: None,
                    }),
                )
                .await;
                hidden(&log, &mut view, &compact, accepted(&secret)).await;
                InvocationOutcome::ContextCompactFinished {
                    outcome: CompactOutcome::Unchanged {
                        reason: "fixture".into(),
                    },
                }
            }
            "interrupted" => {
                hidden(
                    &log,
                    &mut view,
                    &compact,
                    Fact::ModelInterrupted {
                        step_id: "summary".into(),
                        status: ModelInterruption::Cancelled,
                    },
                )
                .await;
                InvocationOutcome::Cancelled {
                    source: "user".into(),
                }
            }
            _ => InvocationOutcome::Failed {
                class: "provider".into(),
                message: None,
            },
        };
        let status = outcome.status();
        let terminal = hidden(
            &log,
            &mut view,
            &compact,
            Fact::InvocationEnded {
                outcome: outcome.clone(),
            },
        )
        .await;
        let ended = log
            .observe_session::<Value>("session")
            .await
            .unwrap()
            .unwrap();
        assert!(ended.active_streams.is_empty());
        assert!(
            matches!(ended.root_turn.unwrap().state, InvocationState::Ended {
            event_id, outcome: actual
        } if event_id == terminal.event.id && actual == outcome)
        );
        assert_eq!(ended.session.read_state, previous.read_state);
        let execution = ended.session.execution.as_ref().unwrap();
        assert!(
            matches!(&execution.state, SessionExecutionState::Ended { status: actual, .. }
            if *actual == status)
        );
        assert_eq!(
            execution.last_message,
            previous.execution.as_ref().unwrap().last_message
        );
        let page = log
            .session_stream_events("session", old_end.sequence, terminal.sequence, 1, 4096)
            .await
            .unwrap();
        assert_eq!(page.events.len(), 1);
        assert!(matches!(
            page.events[0].fact,
            StreamFact::InvocationEnded { .. }
        ));
        assert_eq!(page.events[0].id, terminal.event.id);
        assert!(page.next_after.is_none());
        assert_eq!(page.through_sequence, terminal.sequence);
        assert!(
            log.prepare_transcript("session", terminal.sequence, 1)
                .await
                .unwrap()
        );
        assert_eq!(transcript(&db), old_rows);
        let acknowledged = log
            .set_session_read_marker::<Value>("session", &opened.event.id)
            .await
            .unwrap();
        assert_eq!(acknowledged.read_state, previous.read_state);
        let canonical = log.prefix(100, 1024 * 1024).await.unwrap().digest;
        // Reopening rebuilds the discarded projection, preserving canonical bytes.
        db.execute("DELETE FROM catalog_messages", []).unwrap();
        db.execute("DELETE FROM transcript_rows", []).unwrap();
        db.execute("DELETE FROM transcript_progress", []).unwrap();
        log.close().await.unwrap();
        let log = EventLog::open(&path).await.unwrap();
        let rebuilt = log.get_session::<Value>("session").await.unwrap().unwrap();
        assert_eq!(rebuilt.read_state, previous.read_state);
        assert_eq!(
            rebuilt.execution.as_ref().unwrap().last_message,
            previous.execution.as_ref().unwrap().last_message
        );
        assert!(
            log.prepare_transcript("session", terminal.sequence, 32)
                .await
                .unwrap()
        );
        assert_eq!(transcript(&db), old_rows);
        assert_eq!(
            log.prefix(100, 1024 * 1024).await.unwrap().digest,
            canonical
        );
    }
}
