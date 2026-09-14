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
use maka_protocol::{subscription::*, turn::decode_turn_snapshot};
use serde_json::{Value, json};

#[test]
fn policies_and_close_are_exact_and_follow_js_number_and_id_rules() {
    for policy in [
        json!({"kind":"none"}),
        json!({"kind":"tail","maxBytes":2.0}),
        json!({"kind":"tail","maxBytes":16384}),
    ] {
        assert!(
            decode_subscription_open_input(&json!({"sessionId":"s","transcript":policy})).is_ok()
        );
    }
    for policy in [
        Value::Null,
        json!({"kind":"none","maxBytes":2}),
        json!({"kind":"tail","maxBytes":1}),
        json!({"kind":"tail","maxBytes":16385}),
        json!({"kind":"tail","maxBytes":2.5}),
        json!({"kind":"tail"}),
    ] {
        assert!(
            decode_subscription_open_input(&json!({"sessionId":"s","transcript":policy})).is_err()
        );
    }
    assert!(
        decode_subscription_open_input(&json!({"sessionId":"💬","transcript":{"kind":"none"}}))
            .is_err()
    );
    for length in [1, 64] {
        let input = json!({"subscriptionId":"💬".repeat(length)});
        assert_eq!(
            serde_json::to_value(decode_subscription_close_input(&input).unwrap()).unwrap(),
            input
        );
        assert!(decode_subscription_close_result(&input).is_ok());
    }
    assert!(decode_subscription_close_input(&json!({"subscriptionId":"💬".repeat(65)})).is_err());
    assert!(decode_subscription_close_input(&json!({"subscriptionId":"s","extra":true})).is_err());
}

fn delta() -> Value {
    json!({"kind":"text","turnId":"t","runId":"r","messageId":"m","startOffset":0,"text":"💬"})
}
#[test]
fn deltas_reject_invalid_flags_offsets_and_utf8_byte_overflow() {
    let mut value = delta();
    assert_eq!(
        decode_assistant_delta(&value)
            .unwrap()
            .text
            .encode_utf16()
            .count(),
        2
    );
    for invalid in [json!(false), Value::Null, json!(1)] {
        value["complete"] = invalid.clone();
        assert!(decode_assistant_delta(&value).is_err());
        value.as_object_mut().unwrap().remove("complete");
        value["reset"] = invalid.clone();
        assert!(decode_assistant_delta(&value).is_err());
        value.as_object_mut().unwrap().remove("reset");
        value["interrupted"] = invalid.clone();
        assert!(decode_assistant_delta(&value).is_err());
        value.as_object_mut().unwrap().remove("interrupted");
    }
    value["interrupted"] = json!(true);
    assert!(
        decode_assistant_delta(&value).is_err(),
        "interrupted requires completion"
    );
    value["complete"] = json!(true);
    assert!(decode_assistant_delta(&value).is_ok());
    value.as_object_mut().unwrap().remove("complete");
    value.as_object_mut().unwrap().remove("interrupted");
    value["text"] = json!("");
    assert!(decode_assistant_delta(&value).is_err());
    value["complete"] = json!(true);
    value["startOffset"] = json!(2.0);
    assert_eq!(decode_assistant_delta(&value).unwrap().start_offset, 2);
    value["reset"] = json!(true);
    assert!(decode_assistant_delta(&value).is_err());
    value["startOffset"] = json!(0);
    assert!(decode_assistant_delta(&value).is_ok());
    value["text"] = json!("💬".repeat(4097));
    assert!(decode_assistant_delta(&value).is_err());
    value["text"] = json!("💬".repeat(4096));
    assert!(decode_assistant_delta(&value).is_ok());
    value["startOffset"] = json!(9_007_199_254_740_992_u64);
    assert!(decode_assistant_delta(&value).is_err());
}

#[test]
fn active_streams_require_unique_identity_and_matching_live_root() {
    let root =
        decode_turn_snapshot(&json!({"sessionId":"s","turnId":"t","runId":"r","status":"running"}))
            .unwrap();
    let streams = json!([{"kind":"text","turnId":"t","messageId":"m"},
        {"kind":"thinking","turnId":"t","messageId":"m"}]);
    assert!(decode_active_assistant_streams(&streams, Some(&root)).is_ok());
    assert!(decode_active_assistant_streams(&streams, None).is_err());
    assert!(
        decode_active_assistant_streams(&json!([streams[0], streams[0]]), Some(&root)).is_err()
    );
    let terminal = decode_turn_snapshot(&json!({"sessionId":"s","turnId":"t","runId":"r","status":"completed","terminalEventId":"e"})).unwrap();
    assert!(decode_active_assistant_streams(&streams, Some(&terminal)).is_err());
    assert!(decode_active_assistant_streams(&json!([]), Some(&terminal)).is_ok());
}

#[test]
fn frame_caps_apply_to_encoded_json_and_sequences_are_positive() {
    let mut frame = json!({"kind":"subscription.session_delta","hostEpoch":"e","subscriptionId":"sub","sequence":1,"sessionId":"s","delta":delta()});
    assert!(decode_assistant_observation_frame(&frame).is_ok());
    frame["delta"]["text"] = json!("\u{0000}".repeat(16384));
    assert!(decode_assistant_delta(&frame["delta"]).is_ok());
    assert!(decode_assistant_observation_frame(&frame).is_err());
    let mut closed = json!({"kind":"subscription.closed","hostEpoch":"e","subscriptionId":"sub","sequence":1,"reason":"slow_consumer"});
    assert!(decode_assistant_observation_frame(&closed).is_ok());
    closed["sequence"] = json!(0);
    assert!(decode_assistant_observation_frame(&closed).is_err());
    closed["sequence"] = json!(1);
    closed["reason"] = json!("unknown");
    assert!(decode_assistant_observation_frame(&closed).is_err());
}
