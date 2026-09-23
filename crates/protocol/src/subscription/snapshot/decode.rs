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

//! Inbound observation codecs. Validate required/null fields and raw wire budgets
//! before constructing the Host's existing projection types.
use super::*;
use crate::{codec, interaction, transcript};
use serde_json::Value;

fn budget(value: &Value, limit: usize) -> Result<()> {
    ensure(
        serde_json::to_vec(value).map_err(invalid)?.len() <= limit,
        "Observation exceeds byte limit",
    )
}

fn record(value: &Value, fields: &[&str]) -> Result<()> {
    codec::exact(codec::record(value, "session observation")?, fields)
}

fn string(value: &Value) -> Result<String> {
    let text = value
        .as_str()
        .ok_or_else(|| ProtocolError::invalid("Expected observation ID"))?;
    id(text)?;
    Ok(text.to_owned())
}

pub fn decode_session_observation_snapshot(value: &Value) -> Result<SessionObservationSnapshot> {
    budget(value, 56 * 1024)?;
    record(
        value,
        &[
            "schemaVersion",
            "session",
            "projectionRevision",
            "rootTurn",
            "goal",
            "queue",
            "interactions",
        ],
    )?;
    ensure(
        codec::count(&value["schemaVersion"], "schemaVersion")? == 5,
        "Unsupported observation schema",
    )?;
    let session = &value["session"];
    record(
        session,
        &[
            "sessionId",
            "metadataRevision",
            "status",
            "createdAt",
            "isArchived",
        ],
    )?;
    let session = SessionObservationIdentity {
        session_id: string(&session["sessionId"])?,
        metadata_revision: codec::count(&session["metadataRevision"], "metadataRevision")?,
        status: serde_json::from_value(session["status"].clone()).map_err(invalid)?,
        created_at: codec::count(&session["createdAt"], "createdAt")?,
        is_archived: session["isArchived"]
            .as_bool()
            .ok_or_else(|| ProtocolError::invalid("Expected archive flag"))?,
    };
    let interactions =
        interaction::decode_session_projection(&value["interactions"], &session.session_id)?;
    let snapshot = SessionObservationSnapshot {
        session,
        projection_revision: codec::count(&value["projectionRevision"], "projectionRevision")?,
        root_turn: if value["rootTurn"].is_null() {
            None
        } else {
            Some(decode_turn_snapshot(&value["rootTurn"])?)
        },
        goal: if value["goal"].is_null() {
            None
        } else {
            Some(decode_goal(&value["goal"])?)
        },
        queue: decode_queue(&value["queue"])?,
        interactions,
    };
    snapshot.validate()?;
    Ok(snapshot)
}

pub fn decode_subscription_open_result(value: &Value) -> Result<SubscriptionOpenResult> {
    budget(value, 92 * 1024)?;
    record(
        value,
        &[
            "hostEpoch",
            "subscriptionId",
            "nextSequence",
            "snapshot",
            "activeAssistantStreams",
            "transcript",
        ],
    )?;
    let snapshot = decode_session_observation_snapshot(&value["snapshot"])?;
    let result = SubscriptionOpenResult {
        host_epoch: string(&value["hostEpoch"])?,
        subscription_id: string(&value["subscriptionId"])?,
        next_sequence: codec::count(&value["nextSequence"], "nextSequence")?,
        active_assistant_streams: decode_active_assistant_streams(
            &value["activeAssistantStreams"],
            snapshot.root_turn.as_ref(),
        )?,
        snapshot,
        transcript: if value["transcript"].is_null() {
            None
        } else {
            Some(transcript::decode_session_transcript_bootstrap(
                &value["transcript"],
            )?)
        },
    };
    count(result.next_sequence, true)?;
    ensure(
        result.host_epoch == result.snapshot.queue.host_epoch,
        "Subscription queue epoch mismatch",
    )?;
    Ok(result)
}

pub fn decode_session_projection_frame(value: &Value) -> Result<SessionProjectionFrame> {
    budget(value, SUBSCRIPTION_FRAME_MAX_BYTES)?;
    record(
        value,
        &[
            "kind",
            "hostEpoch",
            "subscriptionId",
            "sequence",
            "snapshot",
        ],
    )?;
    ensure(
        value["kind"] == "subscription.session_projection",
        "Invalid projection frame kind",
    )?;
    let frame = SessionProjectionFrame::SessionProjection {
        host_epoch: string(&value["hostEpoch"])?,
        subscription_id: string(&value["subscriptionId"])?,
        sequence: codec::count(&value["sequence"], "sequence")?,
        snapshot: decode_session_observation_snapshot(&value["snapshot"])?,
    };
    frame.validate()?;
    Ok(frame)
}

pub fn decode_transcript_advanced_frame(value: &Value) -> Result<TranscriptAdvancedFrame> {
    budget(value, SUBSCRIPTION_FRAME_MAX_BYTES)?;
    record(
        value,
        &[
            "kind",
            "hostEpoch",
            "subscriptionId",
            "sequence",
            "sessionId",
            "throughSequence",
        ],
    )?;
    ensure(
        value["kind"] == "subscription.transcript_advanced",
        "Invalid transcript notification kind",
    )?;
    let frame = TranscriptAdvancedFrame::TranscriptAdvanced {
        host_epoch: string(&value["hostEpoch"])?,
        subscription_id: string(&value["subscriptionId"])?,
        sequence: codec::count(&value["sequence"], "sequence")?,
        session_id: string(&value["sessionId"])?,
        through_sequence: codec::count(&value["throughSequence"], "throughSequence")?,
    };
    frame.validate()?;
    Ok(frame)
}

fn decode_goal(value: &Value) -> Result<GoalProjection> {
    record(
        value,
        &[
            "goalId",
            "revision",
            "sessionId",
            "condition",
            "status",
            "setAt",
            "iterations",
            "maxIterations",
            "consecutiveNoProgress",
            "blockCap",
            "tokenBudget",
            "tokensSpent",
            "lastReason",
            "achievedAt",
            "pausedAt",
        ],
    )?;
    // This schema has required nullable fields; the non-null subscription-input
    // normalizer must not be reused here.
    let mut normalized = value.clone();
    for key in [
        "revision",
        "setAt",
        "iterations",
        "maxIterations",
        "consecutiveNoProgress",
        "blockCap",
        "tokensSpent",
    ] {
        normalized[key] = Value::from(codec::count(&value[key], key)?);
    }
    for key in ["tokenBudget", "achievedAt", "pausedAt"] {
        if !value[key].is_null() {
            normalized[key] = Value::from(codec::count(&value[key], key)?);
        }
    }
    let goal: GoalProjection = serde_json::from_value(normalized).map_err(invalid)?;
    goal.validate()?;
    Ok(goal)
}

fn decode_queue(value: &Value) -> Result<SessionMessageQueueProjection> {
    budget(value, 52 * 1024)?;
    record(
        value,
        &["hostEpoch", "queueRevision", "steering", "followup"],
    )?;
    let rows = |key: &str| {
        value[key]
            .as_array()
            .filter(|rows| rows.len() <= 64)
            .ok_or_else(|| ProtocolError::invalid("Invalid queue rows"))
    };
    let decode_row = |value: &Value, placement: &str| -> Result<(QueueMessage, SteeringState)> {
        record(
            value,
            &["entryId", "messageId", "content", "placement", "state"],
        )?;
        ensure(value["placement"] == placement, "Invalid queue placement")?;
        let state = match value["state"].as_str() {
            Some("queued") => SteeringState::Queued,
            Some("in_flight") if placement == "current_turn" => SteeringState::InFlight,
            _ => return Err(ProtocolError::invalid("Invalid queue state")),
        };
        let message = QueueMessage {
            entry_id: string(&value["entryId"])?,
            message_id: string(&value["messageId"])?,
            // Unlike admission, observations may contain Host-owned context refs.
            content: crate::turn::decode(&value["content"])?,
        };
        Ok((message, state))
    };
    let queue = SessionMessageQueueProjection {
        host_epoch: string(&value["hostEpoch"])?,
        queue_revision: codec::count(&value["queueRevision"], "queueRevision")?,
        steering: rows("steering")?
            .iter()
            .map(|row| {
                let (message, state) = decode_row(row, "current_turn")?;
                Ok(SteeringMessageSnapshot::new(message, state))
            })
            .collect::<Result<_>>()?,
        followup: rows("followup")?
            .iter()
            .map(|row| {
                let (message, _) = decode_row(row, "next_turn")?;
                Ok(FollowupMessageSnapshot::new(message))
            })
            .collect::<Result<_>>()?,
    };
    queue.validate()?;
    Ok(queue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> Value {
        json!({
            "schemaVersion":5.0,
            "session":{"sessionId":"s","metadataRevision":1,"status":"active","createdAt":0,"isArchived":false},
            "projectionRevision":1,
            "rootTurn":null,"goal":null,
            "queue":{"hostEpoch":"epoch","queueRevision":0,"steering":[],"followup":[]},
            "interactions":{"pending":[]}
        })
    }

    #[test]
    fn inbound_snapshot_preserves_contract_and_rejects_missing_nullables_or_wrong_owner() {
        let mut value = snapshot();
        let decoded = decode_session_observation_snapshot(&value).unwrap();
        assert_eq!(decoded.session.session_id, "s");
        value["schemaVersion"] = json!(5);
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
        for key in ["rootTurn", "goal", "queue", "interactions"] {
            let mut bad = value.clone();
            bad.as_object_mut().unwrap().remove(key);
            assert!(decode_session_observation_snapshot(&bad).is_err(), "{key}");
        }
        value["rootTurn"] =
            json!({"sessionId":"other","turnId":"t","runId":"r","status":"running"});
        assert!(decode_session_observation_snapshot(&value).is_err());
        value["rootTurn"]["sessionId"] = json!("s");
        decode_session_observation_snapshot(&value).unwrap();
        value["projectionRevision"] = json!(0);
        assert!(decode_session_observation_snapshot(&value).is_err());
    }

    #[test]
    fn open_and_frames_enforce_epoch_sequence_queue_and_raw_budget() {
        let mut open = json!({"hostEpoch":"epoch","subscriptionId":"sub","nextSequence":1.0,
            "snapshot":snapshot(),"activeAssistantStreams":[],"transcript":null});
        let result = decode_subscription_open_result(&open).unwrap();
        result
            .validate_for(
                &SubscriptionOpenInput {
                    session_id: "s".into(),
                    transcript: TranscriptPolicy::None,
                },
                "epoch",
            )
            .unwrap();
        open["snapshot"]["queue"]["hostEpoch"] = json!("other");
        assert!(decode_subscription_open_result(&open).is_err());
        open["snapshot"]["queue"]["hostEpoch"] = json!("epoch");
        open["activeAssistantStreams"] = json!([{"kind":"text","turnId":"t","messageId":"m"}]);
        assert!(decode_subscription_open_result(&open).is_err());
        let mut frame = json!({"kind":"subscription.session_projection","hostEpoch":"epoch","subscriptionId":"sub","sequence":1,"snapshot":snapshot()});
        decode_session_projection_frame(&frame).unwrap();
        frame["snapshot"]["queue"]["followup"] = json!([{"entryId":"e","messageId":"m","content":{"text":"hello"},"placement":"next_turn","state":"in_flight"}]);
        assert!(decode_session_projection_frame(&frame).is_err());
        frame["snapshot"]["queue"]["followup"][0]["state"] = json!("queued");
        decode_session_projection_frame(&frame).unwrap();
        let content = &mut frame["snapshot"]["queue"]["followup"][0]["content"];
        *content = json!({"text":"","attachments":[{"kind":"image","name":"image.png","mimeType":"image/png","bytes":1,
            "ref":{"kind":"workspace_file","relativePath":"image.png"}}]});
        decode_session_projection_frame(&frame).unwrap();
        frame["snapshot"]["queue"]["followup"][0]["content"]["attachments"][0]["ref"] =
            json!({"kind":"session_context","sessionId":"s","refId":"context"});
        assert!(
            decode_session_projection_frame(&frame).is_err(),
            "queues cannot claim Host-owned content"
        );
        frame["snapshot"]["queue"]["followup"][0]["content"] = json!({"text":""});
        assert!(
            decode_session_projection_frame(&frame).is_err(),
            "empty queue message"
        );
        frame["snapshot"]["queue"]["followup"][0]["content"]["text"] = json!("\0".repeat(10_000));
        assert!(decode_session_projection_frame(&frame).is_err());
        let advanced = json!({"kind":"subscription.transcript_advanced","hostEpoch":"epoch","subscriptionId":"sub","sequence":1,"sessionId":"s","throughSequence":0});
        decode_transcript_advanced_frame(&advanced).unwrap();
        let mut bad = advanced;
        bad["sequence"] = json!(9_007_199_254_740_992_u64);
        assert!(decode_transcript_advanced_frame(&bad).is_err());
    }
}
