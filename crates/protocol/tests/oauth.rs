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
fn oauth_contract_matches_original_client_for_all_providers_and_closed_phases() {
    let mut cases = Vec::new();
    for provider in [
        Provider::OpenaiCodex,
        Provider::GithubCopilot,
        Provider::XaiOauth,
    ] {
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
        let start =
            json!({"attemptId":"attempt_1","target":{"kind":"create","providerType":provider}});
        case(&mut cases, Operation::OauthLoginStart, false, start.clone());
        for (key, values) in [
            (
                "slug",
                vec![
                    json!("chosen-slug"),
                    json!("Bad-slug"),
                    json!(""),
                    Value::Null,
                ],
            ),
            (
                "name",
                vec![
                    json!("My subscription"),
                    json!(""),
                    json!("😀".repeat(128)),
                    json!("😀".repeat(129)),
                    Value::Null,
                ],
            ),
        ] {
            for value in values {
                let mut custom = start.clone();
                custom["target"][key] = value;
                case(&mut cases, Operation::OauthLoginStart, false, custom);
            }
        }
        let connection =
            json!({"connectionId":"connection_1","slug":"my-subscription","providerType":provider});
        for phase in [
            "awaiting_authorization",
            "exchanging",
            "committing",
            "authenticated",
            "cancelled",
        ] {
            let projection = json!({"attemptId":"attempt_1","connection":connection,"phase":phase});
            for operation in [
                Operation::OauthLoginStart,
                Operation::OauthLoginQuery,
                Operation::OauthLoginCancel,
            ] {
                case(&mut cases, operation, true, projection.clone());
            }
            pairing(&mut cases, &start, &projection);
            let mut invalid = projection.clone();
            invalid["failure"] = json!("authorization_failed");
            case(&mut cases, Operation::OauthLoginStart, true, invalid);
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
        ] {
            case(
                &mut cases,
                Operation::OauthLoginQuery,
                true,
                json!({"attemptId":"attempt_1","connection":connection,"phase":"failed","failure":failure}),
            );
        }
        let mut wrong =
            json!({"attemptId":"attempt_2","connection":connection,"phase":"authenticated"});
        pairing(&mut cases, &start, &wrong);
        wrong["attemptId"] = json!("attempt_1");
        wrong["connection"]["providerType"] = json!(if provider == Provider::XaiOauth {
            Provider::OpenaiCodex
        } else {
            Provider::XaiOauth
        });
        pairing(&mut cases, &start, &wrong);
        let existing = json!({"attemptId":"attempt_1","target":{"kind":"existing","connectionId":"connection_1"}});
        pairing(&mut cases, &existing, &wrong); // Existing targets bind identity, not an inferred provider.
        wrong["connection"]["connectionId"] = json!("connection_2");
        pairing(&mut cases, &existing, &wrong);
        if provider == Provider::OpenaiCodex {
            let mut custom = start.clone();
            custom["target"]["slug"] = json!("chosen-slug");
            wrong["connection"]["providerType"] = json!(provider);
            pairing(&mut cases, &custom, &wrong);
            wrong["connection"]["slug"] = json!("chosen-slug");
            pairing(&mut cases, &custom, &wrong);
        }
    }
    for value in [
        json!(["ok", {"kind":"create","providerType":"openai-codex"}]),
        json!({"attemptId":"","target":{"kind":"create","providerType":"openai-codex"}}),
        json!({"attemptId":"../id","target":{"kind":"create","providerType":"openai-codex"}}),
        json!({"attemptId":"a".repeat(129),"target":{"kind":"create","providerType":"openai-codex"}}),
        json!({"attemptId":"ok","target":{"kind":"create","providerType":"future-oauth"}}),
        json!({"attemptId":"ok","target":{"kind":"create","providerType":"openai-codex","connectionId":"wrong"}}),
        json!({"attemptId":"ok","target":{"kind":"existing","connectionId":"short-id"}}),
        json!({"attemptId":"ok","target":{"kind":"existing","connectionId":null}}),
        json!({"attemptId":"ok","target":{"kind":"existing","connectionId":"id"},"extra":true}),
    ] {
        case(&mut cases, Operation::OauthLoginStart, false, value);
    }
    for value in [
        json!(["ok"]),
        json!({"attemptId":"a".repeat(128)}),
        json!({"attemptId":"😀"}),
        json!({"attemptId":"ok","extra":true}),
        json!({"attemptId":null}),
    ] {
        case(&mut cases, Operation::OauthLoginQuery, false, value.clone());
        case(&mut cases, Operation::OauthLoginCancel, false, value);
    }
    let base = json!({"attemptId":"ok","connection":{"connectionId":"id","slug":"ok","providerType":"xai-oauth"},"phase":"authenticated"});
    for (pointer, replacement) in [
        ("/connection", json!(["id", "ok", "xai-oauth"])),
        ("/phase", json!("future")),
        ("/phase", json!("failed")),
        ("/connection/slug", json!("a")),
        ("/connection/slug", json!("Bad-slug")),
        ("/connection/providerType", json!("future-oauth")),
        ("/connection/connectionId", json!("../id")),
    ] {
        let mut value = base.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        case(&mut cases, Operation::OauthLoginQuery, true, value);
    }
    for value in [
        json!(["openai-codex", true]),
        json!({"provider":"openai-codex","enabled":null}),
        json!({"provider":"xai-oauth","enabled":true,"extra":1}),
        json!({"provider":"future","enabled":true}),
    ] {
        case(&mut cases, Operation::OauthEnrollmentQuery, true, value);
    }
    case(
        &mut cases,
        Operation::OauthEnrollmentQuery,
        false,
        json!(["openai-codex"]),
    );
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
