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
use maka_protocol::subscription::*;
use serde_json::{Value, json};

fn start() -> Value {
    json!({"type":"tool_start","id":"e:1","turnId":"turn_1","ts":0,
        "toolUseId":"provider:call","toolName":"Read","stepId":"assistant_1"})
}
fn result() -> Value {
    json!({"type":"tool_result","id":"e:2","turnId":"turn_1","ts":1,
        "toolUseId":"provider:call","status":"completed"})
}
fn frame(event: Value) -> Value {
    json!({"kind":"subscription.session_event","hostEpoch":"epoch:1",
        "subscriptionId":"sub:1","sequence":1,"sessionId":"session_1",
        "runId":"run_1","event":event})
}

#[test]
fn outbound_frames_roundtrip_with_exact_identity_and_closed_status() {
    for event in [start(), result()] {
        let value = frame(event);
        let decoded = decode_tool_observation_frame(&value).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
    let mut event = result();
    event["status"] = json!("errored");
    event["operationId"] = json!("operation_1");
    assert!(matches!(
        decode_session_tool_event(&event).unwrap(),
        SessionToolEvent::ToolResult {
            status: ToolResultStatus::Errored,
            ..
        }
    ));
    for status in [json!("failed"), json!("running"), Value::Null, json!(false)] {
        event["status"] = status;
        assert!(decode_session_tool_event(&event).is_err());
    }
}

#[test]
fn live_subset_rejects_durable_fields_and_unimplemented_optional_features() {
    for event in [start(), result()] {
        for key in [
            "parentToolCallId",
            "parentOperationId",
            "origin",
            "modelVisibility",
            "args",
            "content",
            "argsPreview",
            "activityKind",
            "durationMs",
        ] {
            let mut invalid = event.clone();
            invalid[key] = json!("unavailable");
            assert!(
                decode_session_tool_event(&invalid).is_err(),
                "accepted {key}"
            );
        }
    }
    let mut value = frame(start());
    value["parentOperationId"] = json!("operation_1");
    assert!(decode_tool_observation_frame(&value).is_err());
    for key in ["id", "turnId", "ts", "toolUseId", "toolName"] {
        let mut event = start();
        event.as_object_mut().unwrap().remove(key);
        assert!(decode_session_tool_event(&event).is_err(), "missing {key}");
    }
}

#[test]
fn ids_obey_distinct_entity_and_utf16_rules_and_names_use_utf8_bytes() {
    for (key, invalid) in [
        ("id", "".into()),
        ("toolUseId", "💬".repeat(129)),
        ("turnId", "turn:1".into()),
        ("operationId", "step:call".into()),
        ("stepId", " trailing ".into()),
        ("toolName", "💬".repeat(65)),
    ] {
        let mut event = start();
        event[key] = json!(invalid);
        assert!(decode_session_tool_event(&event).is_err(), "accepted {key}");
    }
    let mut event = start();
    event["id"] = json!("💬".repeat(64));
    event["toolUseId"] = json!("💬".repeat(128));
    event["stepId"] = json!("provider:opaque/id");
    event["toolName"] = json!("💬".repeat(64));
    assert!(decode_session_tool_event(&event).is_ok());
    event["stepId"] = Value::Null;
    assert!(decode_session_tool_event(&event).is_err());
    for (key, invalid) in [
        ("hostEpoch", ""),
        ("subscriptionId", ""),
        ("sessionId", "session:1"),
        ("runId", "run:1"),
    ] {
        let mut value = frame(start());
        value[key] = json!(invalid);
        assert!(
            decode_tool_observation_frame(&value).is_err(),
            "accepted {key}"
        );
    }
}

#[test]
fn timestamp_and_sequence_follow_javascript_safe_integer_contract() {
    for number in [json!(-1), json!(1.5), json!(9_007_199_254_740_992_u64)] {
        let mut value = frame(start());
        value["sequence"] = number.clone();
        assert!(decode_tool_observation_frame(&value).is_err());
        value["sequence"] = json!(1);
        value["event"]["ts"] = number;
        assert!(decode_tool_observation_frame(&value).is_err());
    }
    let mut value = frame(start());
    value["sequence"] = json!(0);
    assert!(decode_tool_observation_frame(&value).is_err());
    value["sequence"] = json!(1.0);
    value["event"]["ts"] = json!(9_007_199_254_740_991_u64);
    assert!(decode_tool_observation_frame(&value).is_ok());
    value["padding"] = json!("x".repeat(SUBSCRIPTION_FRAME_MAX_BYTES));
    let error = decode_tool_observation_frame(&value).unwrap_err();
    assert!(error.to_string().contains("exceeds byte limit"));
}
