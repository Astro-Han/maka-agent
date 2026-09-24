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

use maka_protocol::{Operation, oauth::*};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn authentication_contract_matches_client_for_arbitrary_provider_identity_and_receipts() {
    let mut cases = Vec::new();
    for name in ["api-key", "subscription"] {
        let provider = json!({"packageId":"external.providers","entryId":"providers","scope":"profile","name":name});
        let connection = json!({"connectionId":"123e4567-e89b-42d3-a456-426614174000","slug":"personal","provider":provider});
        for enabled in [true, false] {
            case(
                &mut cases,
                Operation::OauthEnrollmentQuery,
                false,
                json!({"provider":provider}),
            );
            case(
                &mut cases,
                Operation::OauthEnrollmentQuery,
                true,
                json!({"provider":provider,"enabled":enabled}),
            );
        }
        for target in [
            json!({"kind":"create","provider":provider,"configuration":{"endpoint":"opaque"},"slug":"personal","name":"Personal"}),
            json!({"kind":"existing","expected":{"connectionId":connection["connectionId"],"revision":1,
                "slug":"personal","provider":provider,"configuration":{"endpoint":"opaque"}},"configuration":{"endpoint":"updated"}}),
        ] {
            let start = json!({"attemptId":"attempt_1","target":target,"authentication":{"method":name,"input":{"key":"transient-secret"}}});
            case(&mut cases, Operation::OauthLoginStart, false, start.clone());
            for phase in [
                "awaiting_authorization",
                "exchanging",
                "committing",
                "authenticated",
                "cancelled",
            ] {
                let projection =
                    json!({"attemptId":"attempt_1","connection":connection,"phase":phase});
                case(
                    &mut cases,
                    Operation::OauthLoginQuery,
                    true,
                    projection.clone(),
                );
                pairing(&mut cases, &start, &projection);
                let mut failed = projection.clone();
                failed["failure"] = json!("provider_rejected");
                case(&mut cases, Operation::OauthLoginQuery, true, failed);
            }
            for field in ["packageId", "entryId", "scope", "name"] {
                let mut wrong = json!({"attemptId":"attempt_1","connection":connection,"phase":"authenticated"});
                wrong["connection"]["provider"][field] = json!(if field == "scope" {
                    "session:other"
                } else {
                    "other"
                });
                pairing(&mut cases, &start, &wrong);
            }
            for failure in [
                "capability_unavailable",
                "authorization_failed",
                "provider_rejected",
                "slug_taken",
                "credential_changed",
                "connection_changed",
                "persistence_failed",
                "internal_failure",
                "outcome_unknown",
            ] {
                case(
                    &mut cases,
                    Operation::OauthLoginQuery,
                    true,
                    json!({
                        "attemptId":"attempt_1","connection":connection,"phase":"failed","failure":failure
                    }),
                );
            }
            for (pointer, value) in [
                ("/attemptId", json!("../invalid")),
                ("/authentication/method", json!("invalid method")),
                ("/authentication/input", json!("x".repeat(65537))),
                ("/target/configuration", json!(null)),
            ] {
                let mut invalid = start.clone();
                *invalid.pointer_mut(pointer).unwrap() = value;
                case(&mut cases, Operation::OauthLoginStart, false, invalid);
            }
        }
    }
    for (method, value) in [
        (
            "open_external",
            json!({"url":"https://example.test","stateHint":"AB-CD"}),
        ),
        ("open_external", json!({"url":"😀".repeat(4096)})),
        ("open_external", json!({"url":"😀".repeat(4096)+"a"})),
        (
            "open_external",
            json!({"url":"url","stateHint":"😀".repeat(512)}),
        ),
        (
            "open_external",
            json!({"url":"url","stateHint":"😀".repeat(513)}),
        ),
        ("open_external", json!({"url":"url","stateHint":null})),
        ("open_external", json!({"url":"","extra":true})),
        ("paste_code", json!({"url":"url"})),
    ] {
        cases.push(json!({"presentation":true,"method":method,"value":value,
            "expected":decode_presentation(method,&value).ok().map(|v|json!(v))}));
    }
    for value in [
        json!({"kind":"presented"}),
        json!({"kind":"presented","extra":1}),
        json!({"kind":"cancelled"}),
    ] {
        cases.push(
            json!({"presentation":true,"output":true,"method":"open_external","value":value,
            "expected":decode_presentation_result("open_external",&value).ok().map(|v|json!(v))}),
        );
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new("node")
        .arg(root.join("crates/protocol/tests/support/oauth_source.mjs"))
        .current_dir(root)
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
}

fn case(cases: &mut Vec<Value>, operation: Operation, output: bool, value: Value) {
    let expected = if output {
        decode_output(operation, &value)
    } else {
        decode_input(operation, &value)
    }
    .ok();
    cases.push(
        json!({"operation":operation,"output":output,"value":value,"expected":expected,
        "errors":errors(operation).unwrap()}),
    );
}
fn pairing(cases: &mut Vec<Value>, input: &Value, output: &Value) {
    let expected = assert_start(
        &decode_start(input).unwrap(),
        &decode_login(output).unwrap(),
    )
    .is_ok();
    cases.push(json!({"pairing":true,"operation":"oauth.login.start","input":input,"value":output,"expected":expected}));
    let attempt = json!({"attemptId":input["attemptId"]});
    let expected = assert_attempt(
        &decode_attempt(&attempt).unwrap(),
        &decode_login(output).unwrap(),
    )
    .is_ok();
    for operation in ["oauth.login.query", "oauth.login.cancel"] {
        cases.push(json!({"pairing":true,"operation":operation,"input":attempt,"value":output,"expected":expected}));
    }
}
