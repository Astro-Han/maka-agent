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

use maka_protocol::{session::SessionStatus, subscription::*, turn::decode_turn_snapshot};
use serde_json::json;

fn snapshot() -> SessionObservationSnapshot {
    SessionObservationSnapshot::new(
        SessionObservationIdentity {
            session_id: "s".into(),
            metadata_revision: 1,
            status: SessionStatus::Active,
            created_at: 123,
            is_archived: false,
        },
        1,
        None,
        None,
        SessionMessageQueueProjection {
            host_epoch: "epoch".into(),
            queue_revision: 0,
            steering: vec![],
            followup: vec![],
        },
        SessionInteractionProjection::default(),
    )
}
fn open() -> SubscriptionOpenResult {
    SubscriptionOpenResult::new("epoch".into(), "sub".into(), 1, snapshot(), vec![], None)
}
fn input() -> SubscriptionOpenInput {
    SubscriptionOpenInput {
        session_id: "s".into(),
        transcript: TranscriptPolicy::None,
    }
}

#[test]
fn none_result_has_required_nulls_and_actual_metadata() {
    let result = open();
    result.validate_for(&input(), "epoch").unwrap();
    let wire = serde_json::to_value(&result).unwrap();
    assert_eq!(
        wire["snapshot"],
        json!({
            "schemaVersion": 5,
            "session": {"sessionId":"s","metadataRevision":1,"status":"active","createdAt":123,"isArchived":false},
            "projectionRevision":1,"rootTurn":null,"goal":null,
            "queue":{"hostEpoch":"epoch","queueRevision":0,"steering":[],"followup":[]},
            "interactions":{"pending":[]}
        })
    );
    assert!(wire.as_object().unwrap().contains_key("transcript"));
    assert!(wire["transcript"].is_null());
}

#[test]
fn open_enforces_request_epoch_identity_and_stream_liveness() {
    let mut result = open();
    assert!(result.validate_for(&input(), "another").is_err());
    let mut request = input();
    request.transcript = TranscriptPolicy::Tail { max_bytes: 100 };
    assert!(result.validate_for(&request, "epoch").is_err());
    result.snapshot.queue.host_epoch = "another".into();
    assert!(result.validate_for(&input(), "epoch").is_err());
    result.snapshot.queue.host_epoch = "epoch".into();
    result
        .active_assistant_streams
        .push(SessionAssistantStreamIdentity {
            kind: AssistantStreamKind::Text,
            turn_id: "t".into(),
            message_id: "m".into(),
        });
    assert!(result.validate_for(&input(), "epoch").is_err());
    result.snapshot.root_turn = Some(
        decode_turn_snapshot(&json!({
            "sessionId":"s","turnId":"t","runId":"r","status":"running"
        }))
        .unwrap(),
    );
    result.validate_for(&input(), "epoch").unwrap();
    result.snapshot.root_turn.as_mut().unwrap().session_id = "other".into();
    assert!(result.validate_for(&input(), "epoch").is_err());
    result.snapshot.root_turn.as_mut().unwrap().session_id = "s".into();
    result.next_sequence = 9_007_199_254_740_992;
    assert!(result.validate_for(&input(), "epoch").is_err());
}

fn message(entry: &str, id: &str) -> QueueMessage {
    QueueMessage {
        entry_id: entry.into(),
        message_id: id.into(),
        content: serde_json::from_value(json!({"text":"hello"})).unwrap(),
    }
}
#[test]
fn queue_rejects_duplicate_messages_across_placements_and_encoded_overflow() {
    let mut queue = snapshot().queue;
    queue.steering.push(SteeringMessageSnapshot::new(
        message("e1", "m"),
        SteeringState::InFlight,
    ));
    queue
        .followup
        .push(FollowupMessageSnapshot::new(message("e2", "m")));
    assert!(queue.validate().is_err());
    queue.followup[0].message.message_id = "m2".into();
    queue.validate().unwrap();
    let wire = serde_json::to_value(&queue).unwrap();
    assert_eq!(wire["steering"][0]["state"], "in_flight");
    assert_eq!(wire["steering"][0]["placement"], "current_turn");
    assert_eq!(wire["followup"][0]["placement"], "next_turn");
    queue.followup[0].message.content.text = "\0".repeat(10_000);
    assert!(queue.validate().is_err());
}

#[test]
fn goal_limits_use_utf16_and_safe_positive_budgets() {
    let mut goal = GoalProjection {
        goal_id: "g".into(),
        revision: 0,
        session_id: "s".into(),
        condition: "💬".repeat(250),
        status: GoalStatus::Active,
        set_at: 0,
        iterations: 0,
        max_iterations: 1,
        consecutive_no_progress: 0,
        block_cap: 1,
        token_budget: None,
        tokens_spent: 0,
        last_reason: None,
        achieved_at: None,
        paused_at: None,
    };
    goal.validate().unwrap();
    goal.condition.push('x');
    assert!(goal.validate().is_err());
    goal.condition.clear();
    goal.token_budget = Some(0);
    assert!(goal.validate().is_err());
    goal.token_budget = Some(1);
    let mut observation = snapshot();
    goal.session_id = "other".into();
    observation.goal = Some(goal);
    assert!(observation.validate().is_err());
    observation.goal.as_mut().unwrap().session_id = "s".into();
    observation.validate().unwrap();
    observation.projection_revision = 0;
    assert!(observation.validate().is_err());
}
