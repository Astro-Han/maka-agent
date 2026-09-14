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

use maka_protocol::turn::*;
use serde_json::{Value, json};

fn input() -> Value {
    json!({"sessionId":"s1","turnId":"t1","content":{"text":"hello"}})
}
fn live() -> Value {
    json!({"sessionId":"s1","turnId":"t1","runId":"r1","status":"running"})
}
fn empty_skills() -> Value {
    json!({"loaded":[],"failed":[],"receipts":[]})
}

#[test]
fn admission_canonicalizes_and_checks_utf16_occurrences() {
    let mut request = input();
    request["maxSteps"] = json!(1.0);
    request["skillIds"] = json!([]);
    request["content"] = json!({"text":"😀 /skill:build","displayText":"😀 /skill:build",
        "attachments":[],"quotes":[],"directoryReferences":[],
        "inlineReferences":[{"kind":"skill","value":"/skill:build","label":"build","start":3}]});
    let decoded = decode_turn_start_input(&request).unwrap();
    assert_eq!(decoded.max_steps, Some(1));
    assert!(decoded.skill_ids.is_none());
    assert!(decoded.content.display_text.is_none());
    assert!(decoded.content.attachments.is_none());
    assert_eq!(decoded.content.inline_references.unwrap()[0].start, 3);
    request["content"]["inlineReferences"][0]["start"] = json!(2);
    assert!(decode_turn_start_input(&request).is_err());
    request = input();
    request["content"]["text"] = json!("");
    assert!(decode_turn_start_input(&request).is_err());
    request["skillIds"] = json!(["plugin:build"]);
    assert!(decode_turn_start_input(&request).is_ok());
    for invalid in [
        json!(0),
        json!(1.5),
        json!(9_007_199_254_740_992u64),
        Value::Null,
    ] {
        request["maxSteps"] = invalid;
        assert!(decode_turn_start_input(&request).is_err());
    }
}

#[test]
fn admission_rejects_authority_claims_paths_unknown_keys_and_encoded_limits() {
    for storage in [
        json!({"kind":"session_context","sessionId":"s1","refId":"r1"}),
        json!({"kind":"workspace_file","relativePath":"../secret"}),
        json!({"kind":"session_file","sessionId":"bad id","relativePath":"file"}),
        json!({"kind":"external_file","absolutePath":"relative"}),
    ] {
        let mut request = input();
        request["content"]["attachments"] =
            json!([{"kind":"other","name":"f","mimeType":"text/plain","bytes":1,"ref":storage}]);
        assert!(decode_turn_start_input(&request).is_err());
    }
    let mut request = input();
    request["content"]["quotes"] = json!([{"text":"ok","label":"😀".repeat(101)}]);
    assert!(decode_turn_start_input(&request).is_err());
    request = input();
    request["content"]["text"] = json!("\n".repeat(30_000));
    assert!(
        decode_turn_start_input(&request).is_err(),
        "Encoded JSON size counts escaped text"
    );
    request = input();
    request["content"]["unknown"] = json!(true);
    assert!(decode_turn_start_input(&request).is_err());
    assert!(
        decode_turn_query_input(&json!({"sessionId":"s1","turnId":"t1","runId":"r1"})).is_err()
    );
    assert!(decode_turn_stop_input(&json!({"sessionId":"s1","turnId":"t1","runId":"r1"})).is_ok());
}

#[test]
fn snapshots_preserve_terminal_evidence_and_retry_constraints() {
    for status in ["admitted", "created", "running", "waiting_for_user"] {
        let mut value = live();
        value["status"] = json!(status);
        assert_eq!(
            serde_json::to_value(decode_turn_snapshot(&value).unwrap()).unwrap(),
            value
        );
        value["terminalEventId"] = json!("event");
        assert!(decode_turn_snapshot(&value).is_err());
    }
    let mut value = live();
    value["providerRetry"] = json!({"phase":"scheduled","attempt":1.0,"maxAttempts":2,"delayMs":0,"ts":2,"reason":"timeout"});
    assert!(decode_turn_snapshot(&value).is_ok());
    value["providerRetry"]["attempt"] = json!(3);
    assert!(decode_turn_snapshot(&value).is_err());
    value = live();
    value["status"] = json!("failed");
    value["terminalEventId"] = json!("terminal:event");
    value["failureClass"] = json!("provider_error");
    value["failureMessage"] = json!("é".repeat(128));
    assert!(decode_turn_snapshot(&value).is_ok());
    value["failureMessage"] = json!("é".repeat(129));
    assert!(decode_turn_snapshot(&value).is_err());
    value.as_object_mut().unwrap().remove("failureMessage");
    value.as_object_mut().unwrap().remove("terminalEventId");
    assert!(decode_turn_snapshot(&value).is_err());
}

#[test]
fn start_results_bind_identity_and_validate_skill_receipts() {
    let input = decode_turn_start_input(&input()).unwrap();
    let mut output = json!({"kind":"started","turn":live(),"skillInvocation":empty_skills()});
    assert!(
        assert_start_output_for_input(&input, &decode_turn_start_result(&output).unwrap()).is_ok()
    );
    output["turn"]["turnId"] = json!("other");
    assert!(
        assert_start_output_for_input(&input, &decode_turn_start_result(&output).unwrap()).is_err()
    );
    let mut blocked = json!({"kind":"blocked","skillInvocation":empty_skills()});
    assert!(decode_turn_start_result(&blocked).is_err());
    blocked["skillInvocation"]["failed"] =
        json!([{"reason":"too_many_requests","requestLimit":50}]);
    blocked["skillInvocation"]["receipts"] = json!([{"invocation":"explicit","success":false,"reason":"too_many_requests","requestLimit":50}]);
    assert!(decode_turn_start_result(&blocked).is_ok());
    blocked["skillInvocation"]["receipts"][0]["invocation"] = json!("model_tool");
    assert!(decode_turn_start_result(&blocked).is_err());
    blocked["skillInvocation"]["receipts"] = json!([]);
    blocked["skillInvocation"]["loaded"] = json!([{"id":"loaded","name":"Loaded"}]);
    assert!(decode_turn_start_result(&blocked).is_err());
}
