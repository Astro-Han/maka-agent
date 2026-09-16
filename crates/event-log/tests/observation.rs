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

use maka_event_log::{EventLog, turns::InvocationState};
use maka_runtime::event::EventWrite;
use maka_runtime::event::{Fact, Invocation, InvocationOutcome, RuntimeEvent};
use maka_runtime::{
    event::{LogScope, ModelInterruption},
    model::{ModelEvent, ModelStep, TextKind},
};
use serde_json::{Value, json};

#[tokio::test]
async fn fenced_bootstrap_and_bounded_catchup_survive_coalescing_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let mut wake = log.subscribe_commits();
    let fresh = log
        .observe_session::<Value>("session")
        .await
        .unwrap()
        .unwrap();
    assert!(fresh.root_turn.is_none());
    assert!(fresh.active_streams.is_empty());
    assert_eq!(fresh.through_sequence, 0);
    log.create_session("idle", "idle-create", &json!({}), 1)
        .await
        .unwrap();
    let targets = ["session".into(), "idle".into()];
    let versions = log.observation_versions(&targets).await.unwrap();
    assert_eq!(
        versions["session"],
        maka_event_log::observation::ObservationVersion {
            metadata: 1,
            queue: 0,
            event: 0
        }
    );
    let first = Invocation {
        session_id: "session".into(),
        turn_id: "first".into(),
        run_id: "run-first".into(),
        invocation_id: "invocation-first".into(),
    };
    let opening = RuntimeEvent::new(
        first.clone(),
        Fact::InvocationOpened {
            configuration: None,
            input: maka_runtime::input::InvocationInput::Message {
                skill_invocation: Default::default(),
                source_messages: Vec::new(),
                content: "one".into(),
                request_fingerprint: None,
            },
        },
    );
    let terminal = RuntimeEvent::new(
        first,
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    );
    log.append_batch(
        &([opening.clone(), terminal.clone()])
            .iter()
            .cloned()
            .map(EventWrite::plain)
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
    )
    .await
    .unwrap();
    let fence = log
        .observe_session::<Value>("session")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fence.session.revision, 3);
    assert!(fence.active_streams.is_empty());
    assert!(matches!(
        fence.root_turn.unwrap().state,
        InvocationState::Ended { .. }
    ));
    let mut second = opening.clone();
    second.id = "second-event".into();
    second.invocation.turn_id = "second".into();
    second.invocation.run_id = "run-second".into();
    second.invocation.invocation_id = "invocation-second".into();
    log.append(&EventWrite::plain((second).clone()).unwrap())
        .await
        .unwrap();
    let mut other = second;
    other.id = "other-event".into();
    other.invocation.session_id = "other".into();
    other.invocation.invocation_id = "invocation-other".into();
    log.append(&EventWrite::plain((other).clone()).unwrap())
        .await
        .unwrap();
    // Several commits collapse to one hint. The stored cursor still covers all facts.
    assert_eq!(*wake.borrow_and_update(), 4);
    let advanced = log.observation_versions(&targets).await.unwrap();
    assert_eq!(
        advanced["idle"], versions["idle"],
        "unrelated commits do not invalidate an idle observer"
    );
    assert_eq!(
        advanced["session"].event, 3,
        "observer versions track their Session, not the global fence"
    );
    log.append(&EventWrite::plain((opening).clone()).unwrap())
        .await
        .unwrap();
    assert!(
        !wake.has_changed().unwrap(),
        "exact replay is not a new delivery"
    );
    let one = log
        .session_events(
            "session",
            fresh.through_sequence,
            fence.through_sequence,
            1,
            2048,
        )
        .await
        .unwrap();
    assert_eq!(one.events[0].event, opening);
    assert_eq!(one.next_after, Some(1));
    let two = log
        .session_events(
            "session",
            one.next_after.unwrap(),
            fence.through_sequence,
            1,
            2048,
        )
        .await
        .unwrap();
    assert_eq!(two.events[0].event, terminal);
    assert!(
        two.next_after.is_none(),
        "later commits cannot enter the old fence"
    );
    assert!(
        log.session_events("session", 0, 4, 1, 1).await.is_err(),
        "oversized first fact cannot be silently skipped"
    );
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    let resumed = log
        .observe_session::<Value>("session")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.through_sequence, 4);
    assert_eq!(resumed.root_turn.unwrap().invocation.turn_id, "second");
    let catchup = log
        .session_events(
            "session",
            fence.through_sequence,
            resumed.through_sequence,
            8,
            4096,
        )
        .await
        .unwrap();
    assert_eq!(catchup.events.len(), 1);
    assert_eq!(catchup.events[0].sequence, 3);
    assert!(catchup.next_after.is_none());
}

#[tokio::test]
async fn active_stream_seeds_are_fenced_bounded_and_survive_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("streams.sqlite");
    let mut log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let invocation = Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    let event = |fact| RuntimeEvent::new(invocation.clone(), fact);
    log.append(
        &EventWrite::plain(
            (event(Fact::InvocationOpened {
                configuration: None,
                input: maka_runtime::input::InvocationInput::Message {
                    skill_invocation: Default::default(),
                    source_messages: Vec::new(),
                    content: "".into(),
                    request_fingerprint: None,
                },
            }))
            .clone(),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    let observed = |step: &str, observation| {
        event(Fact::ModelObserved {
            step_id: step.into(),
            event: observation,
        })
    };
    let start = |step: &str, id: &str| {
        observed(
            step,
            ModelEvent::PartStarted {
                id: id.into(),
                text_kind: TextKind::Thinking,
                provider_options: None,
            },
        )
    };
    let delta = |step: &str, text: &str| {
        observed(
            step,
            ModelEvent::PartDelta {
                id: "same".into(),
                text: text.into(),
                provider_options: None,
            },
        )
    };
    for step in ["interrupted", "current"] {
        log.append(
            &EventWrite::plain(
                (event(Fact::ModelRequested {
                    purpose: None,
                    context: None,
                    checkpoint_event_id: None,
                    step_id: step.into(),
                    model_id: "test".into(),
                    source_scope: LogScope::Root,
                    source_high_water: 0,
                    source_digest: "fixture".into(),
                    effective_source_digest: None,
                    input_digest: "input".into(),
                    route_identity: "route".into(),
                }))
                .clone(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    }
    log.append(&EventWrite::plain((start("interrupted", "same")).clone()).unwrap())
        .await
        .unwrap();
    log.append(&EventWrite::plain((delta("interrupted", "excluded")).clone()).unwrap())
        .await
        .unwrap();
    log.append(
        &EventWrite::plain(
            (event(Fact::ModelInterrupted {
                step_id: "interrupted".into(),
                status: ModelInterruption::Cancelled,
            }))
            .clone(),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    log.append(&EventWrite::plain((start("current", "finished")).clone()).unwrap())
        .await
        .unwrap();
    log.append(
        &EventWrite::plain(
            (observed(
                "current",
                ModelEvent::PartFinished {
                    id: "finished".into(),
                    provider_options: None,
                },
            ))
            .clone(),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    let opening = start("current", "same");
    log.append(&EventWrite::plain((opening).clone()).unwrap())
        .await
        .unwrap();
    log.append(&EventWrite::plain((delta("current", "a😀")).clone()).unwrap())
        .await
        .unwrap();
    let fence = log
        .observe_session::<Value>("session")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fence.active_streams.len(), 2);
    let seed = fence
        .active_streams
        .iter()
        .find(|seed| seed.part_id == "same")
        .unwrap();
    assert_eq!(seed.message_id, opening.id);
    assert_eq!(seed.step_id, "current");
    assert_eq!(seed.part_id, "same");
    assert_eq!(seed.text_kind, TextKind::Thinking);
    assert!(seed.start_sequence < fence.through_sequence);
    log.append(&EventWrite::plain((delta("current", "🎉")).clone()).unwrap())
        .await
        .unwrap();
    log.append(
        &EventWrite::plain(event(Fact::ModelObserved {
            step_id: "current".into(),
            event: ModelEvent::PartFinished {
                id: "same".into(),
                provider_options: None,
            },
        }))
        .unwrap(),
    )
    .await
    .unwrap();
    log.close().await.unwrap();
    log = EventLog::open(&path).await.unwrap();
    let reopened = log
        .observe_session::<Value>("session")
        .await
        .unwrap()
        .unwrap();
    let reopened_seed = reopened
        .active_streams
        .iter()
        .find(|stream| stream.part_id == "same")
        .unwrap();
    assert_eq!(reopened_seed.message_id, seed.message_id);
    assert_eq!(
        reopened_seed.start_sequence, seed.start_sequence,
        "later commits retain the same replay start"
    );
    let catchup = log
        .session_events(
            "session",
            fence.through_sequence,
            reopened.through_sequence,
            8,
            4096,
        )
        .await
        .unwrap();
    assert_eq!(catchup.events.len(), 2);
    assert_eq!(
        reopened.active_streams.len(),
        2,
        "closed part still awaits the request verdict"
    );
    for index in 0..128 {
        log.append(
            &EventWrite::plain((start("current", &format!("extra-{index}"))).clone()).unwrap(),
        )
        .await
        .unwrap();
    }
    assert!(matches!(
        log.observe_session::<Value>("session").await,
        Err(maka_event_log::StoreError::PrefixTooLarge)
    ));
    log.append(
        &EventWrite::plain(
            (event(Fact::ModelCompleted {
                step_id: "current".into(),
                output: ModelStep {
                    parts: vec![],
                    finish_reason: maka_runtime::model::ModelFinishReason::Stop,
                    usage: Default::default(),
                    provider_options: None,
                    response_id: None,
                    model: None,
                    timestamp: None,
                },
            }))
            .clone(),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    assert!(
        log.observe_session::<Value>("session")
            .await
            .unwrap()
            .unwrap()
            .active_streams
            .is_empty()
    );
    log.append(
        &EventWrite::plain(
            (event(Fact::InvocationEnded {
                outcome: InvocationOutcome::Cancelled {
                    source: "user".into(),
                },
            }))
            .clone(),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    assert!(
        log.observe_session::<Value>("session")
            .await
            .unwrap()
            .unwrap()
            .active_streams
            .is_empty()
    );
}
