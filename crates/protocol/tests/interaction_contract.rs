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

use maka_protocol::interaction::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[path = "support/interaction_forms.rs"]
mod forms;
#[path = "support/interaction_questions.rs"]
mod questions;

fn request() -> Value {
    json!({"kind":"client_capability","toolUseId":"tool",
        "target":{"providerId":"provider","contractId":"contract","serverId":"server",
        "toolName":"browser","capability":"browser","scope":{"kind":"browser_origin","origin":"https://example.com"}}})
}
fn pending() -> Value {
    json!({"schemaVersion":1,"sessionId":"session","turnId":"turn","runId":"run",
        "interactionId":"interaction","request":request(),"revision":1,"status":"pending","outcome":null})
}
fn decode(kind: &str, value: &Value) -> maka_protocol::Result<Value> {
    fn wire(value: impl serde::Serialize) -> Value {
        serde_json::to_value(value).unwrap()
    }
    match kind {
        "request" => decode_request(value).map(wire),
        "answer" => decode_answer(value).map(wire),
        "outcome" => decode_outcome(value).map(wire),
        "snapshot" => decode_snapshot(value).map(wire),
        "answered" => decode_answered_snapshot(value).map(wire),
        "projection" => decode_session_projection(value, "session").map(wire),
        "query" => decode_query_input(value).map(wire),
        "answer_input" => decode_answer_input(value).map(wire),
        _ => unreachable!(),
    }
}

#[test]
fn original_typescript_agrees_on_grants_lifecycle_identity_and_bounds() {
    let mut cases = Vec::new();
    let mut add = |kind: &str, input: Value| {
        let expected = match decode(kind, &input) {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(_) => json!({"ok":false}),
        };
        cases.push(json!({"kind":kind,"input":input,"expected":expected}));
    };
    add("request", request());
    let form = forms::cases(&mut add);
    questions::cases(&mut add);
    for origin in [
        "https://example.com",
        "http://localhost:8080",
        "https://[::1]",
        "https://example.com/",
        "https://EXAMPLE.com",
        "https://example.com:443",
        "http://user@example.com",
        "file://example.com",
        "https://例子.测试",
        "http://127.1",
        "https://example.com?",
        "https://example.com#",
    ] {
        let mut value = request();
        value["target"]["scope"]["origin"] = json!(origin);
        add("request", value);
    }
    for (pointer, replacement) in [
        ("/toolUseId", json!("😀".repeat(64))),
        ("/toolUseId", json!("😀".repeat(65))),
        ("/toolUseId", json!("")),
        ("/toolUseId", json!("a\0b")),
        ("/target/providerId", json!("a".repeat(128))),
        ("/target/providerId", json!("a".repeat(129))),
        ("/target/providerId", json!("a.b")),
        ("/target/scope/extra", json!(true)),
        ("/target/extra", json!("x".repeat(20_000))),
        ("/extra", json!(true)),
        ("/target/capability", json!("computer_use")),
    ] {
        let mut value = request();
        // pointer_mut cannot insert a missing key.
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        value
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(key.into(), replacement);
        add("request", value);
    }
    for (capability, scope) in [
        (
            "mcp",
            json!({"kind":"mcp_tool","serverId":"server","toolName":"read"}),
        ),
        (
            "mcp",
            json!({"kind":"browser_origin","origin":"https://example.com"}),
        ),
        ("computer_use", json!({"kind":"capability"})),
        (
            "computer_use",
            json!({"kind":"capability","origin":"https://example.com"}),
        ),
        (
            "desktop_mcp",
            json!({"kind":"mcp_tool","serverId":"different","toolName":"other"}),
        ),
        (
            "desktop_mcp",
            json!({"kind":"mcp_tool","serverId":"bad.id","toolName":"other"}),
        ),
    ] {
        let mut value = request();
        value["target"]["capability"] = json!(capability);
        value["target"]["scope"] = scope;
        add("request", value);
    }
    for length in [16_000, 16_200, 16_384] {
        let mut value = request();
        value["target"]["scope"]["origin"] = json!(format!("https://{}", "a".repeat(length)));
        add("request", value);
    }
    for decision in ["allow", "deny", "unknown"] {
        let answer = json!({"kind":"client_capability","decision":decision});
        add("answer", answer.clone());
        add(
            "answer_input",
            json!({"sessionId":"session","interactionId":"interaction","answer":answer}),
        );
        for timestamp in [
            json!(0),
            json!(1.0),
            json!(-1),
            json!(1.5),
            json!(9_007_199_254_740_991_u64),
            json!(9_007_199_254_740_992_u64),
        ] {
            add(
                "outcome",
                json!({"kind":"client_capability_decision","decision":decision,"committedAt":timestamp}),
            );
        }
    }
    let outcomes = std::iter::once(
        json!({"kind":"client_capability_decision","decision":"allow","committedAt":1}),
    )
    .chain(
        [
            "turn_stopped",
            "turn_terminal",
            "producer_cancelled",
            "timed_out",
            "host_restarted",
            "provider_disconnected",
            "unknown",
        ]
        .into_iter()
        .map(|reason| json!({"kind":"closure","reason":reason,"committedAt":1})),
    )
    .chain(std::iter::once(Value::Null))
    .collect::<Vec<_>>();
    for outcome in &outcomes {
        let mut value = pending();
        value["request"] = form.clone();
        value["status"] = json!("closed");
        value["revision"] = json!(2);
        value["outcome"] = outcome.clone();
        add("snapshot", value);
    }
    for status in ["pending", "answered", "closed", "unknown"] {
        for revision in [1, 2, 3] {
            for outcome in &outcomes {
                let mut value = pending();
                value["status"] = json!(status);
                value["revision"] = json!(revision);
                value["outcome"] = outcome.clone();
                add("snapshot", value.clone());
                add("answered", value);
            }
        }
    }
    add("snapshot", pending());
    for (field, value) in [
        ("schemaVersion", json!(2)),
        ("revision", json!(1.0)),
        ("interactionId", json!("a.b")),
        ("turnId", json!("")),
        ("extra", json!(true)),
    ] {
        let mut snapshot = pending();
        snapshot[field] = value;
        add("snapshot", snapshot);
    }
    for length in [0, 1, 16, 17] {
        let snapshots = (0..length)
            .map(|i| {
                let mut snapshot = pending();
                snapshot["interactionId"] = json!(format!("i{i}"));
                snapshot
            })
            .collect::<Vec<_>>();
        add("projection", json!({"pending":snapshots}));
    }
    add("projection", json!({"pending":[pending(),pending()]}));
    let mut foreign = pending();
    foreign["sessionId"] = json!("other");
    add("projection", json!({"pending":[foreign]}));
    for value in [
        json!({"sessionId":"session","interactionId":"interaction"}),
        json!({"sessionId":"session","interactionId":"a.b"}),
        json!({"sessionId":"session","interactionId":"interaction","extra":true}),
    ] {
        add("query", value);
    }
    let mut child = Command::new("node")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/client-interaction-contract.mjs"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&cases).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("original-client-interaction-contract")
    );
}

#[test]
fn canonical_record_validation_and_projection_derive_resolution() {
    let mut record = InteractionRecord {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        request_id: "interaction".into(),
        created_at: 1,
        request: decode_request(&request()).unwrap(),
        outcome: None,
    };
    assert_eq!(
        serde_json::to_value(InteractionSnapshot::from_record(&record).unwrap()).unwrap(),
        pending()
    );
    record.outcome = Some(InteractionOutcome::Closure {
        reason: ClosureReason::HostRestarted,
        committed_at: 2,
    });
    let wire = serde_json::to_value(InteractionSnapshot::from_record(&record).unwrap()).unwrap();
    assert_eq!(wire["revision"], 2);
    assert_eq!(wire["status"], "closed");
    assert_eq!(
        decode_snapshot(&wire).unwrap(),
        InteractionSnapshot::from_record(&record).unwrap()
    );
    assert!(decode_answered_snapshot(&wire).is_err());
    record.created_at = MAX_SAFE_INTEGER + 1;
    assert!(InteractionSnapshot::from_record(&record).is_err());
    // Malformed questions and unsupported producers remain invalid.
    assert!(decode_request(&json!({"kind":"question","questions":[]})).is_err());
    assert!(decode_answer(&json!({"kind":"permission","decision":"allow"})).is_err());
}
