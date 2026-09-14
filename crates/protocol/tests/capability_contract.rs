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

use maka_protocol::capability::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn manifest() -> Value {
    json!({"registrationId":"registration-1","offers":[{
        "offerId":"desktop","version":"1","affinity":"session","hostPathAccess":"none",
        "label":"桌面","description":"Native tools",
        "tools":[{"serverId":"desktop.native","name":"browser",
            "description":"Inspect the browser",
            "inputSchema":{"type":"object","properties":{"url":{"type":"string","pattern":"(?<=https:)//"}}},
            "activityKind":"computer",
            "annotations":{"title":"Browser","readOnlyHint":true,"destructiveHint":false,
                "idempotentHint":false,"openWorldHint":true}
        }]
    }],"services":[{"serviceId":"oauth_presentation","version":"1"},
        {"serviceId":"maka_scheduled_task_native_effect","version":"1"}]})
}

#[test]
fn manifests_normalize_and_reject_the_same_boundaries_as_the_current_client() {
    let mut cases = Vec::new();
    let mut add =
        |kind: &str, input: Value| {
            let result =
                match kind {
                    "manifest" => decode_replace_input(&input)
                        .map(|value| serde_json::to_value(value).unwrap()),
                    "result" => decode_registration_result(&input)
                        .map(|value| serde_json::to_value(value).unwrap()),
                    "unregister" => decode_unregister_input(&input)
                        .map(|value| serde_json::to_value(value).unwrap()),
                    "schema" => schema::validate(&input).map(|_| input.clone()),
                    "call-result" => {
                        decode_result(&input).map(|value| serde_json::to_value(value).unwrap())
                    }
                    "client-frame" => decode_client_frame(&input)
                        .map(|value| serde_json::to_value(value).unwrap()),
                    "host-frame" => {
                        decode_host_frame(&input).map(|value| serde_json::to_value(value).unwrap())
                    }
                    _ => unreachable!(),
                };
            let expected = match result {
                Ok(value) => json!({"ok":true, "value":value}),
                Err(_) => json!({"ok":false}),
            };
            cases.push(json!({"kind":kind,"input":input,"expected":expected}));
        };
    add("manifest", manifest());
    let mut absent = manifest();
    absent.as_object_mut().unwrap().remove("services");
    add("manifest", absent);
    let mut nullable = manifest();
    nullable["services"] = Value::Null;
    add("manifest", nullable);
    add(
        "manifest",
        json!({"registrationId":"services","offers":[],
        "services":[{"serviceId":"native","version":"1"}]}),
    );
    add(
        "manifest",
        json!({"registrationId":"empty","offers":[],"services":null}),
    );
    for (pointer, replacement) in [
        ("/registrationId", json!("invalid.id")),
        ("/offers/0/description", Value::Null),
        ("/offers/0/tools/0/description", json!("")),
        ("/offers/0/tools/0/annotations/readOnlyHint", Value::Null),
        ("/offers/0/tools/0/activityKind", json!("unknown")),
        (
            "/offers/0/tools/0/inputSchema",
            json!({"type":"object", "$schema":"https://example.invalid"}),
        ),
        (
            "/offers/0/tools/0/inputSchema",
            json!({"type":"object", "$ref":"file:///private"}),
        ),
        (
            "/offers/0/tools/0/inputSchema",
            json!({"type":"object", "properties":{"x":false},
            "required":["x","x"]}),
        ),
        ("/offers/0/label", json!("😀".repeat(65))),
    ] {
        let mut input = manifest();
        *input.pointer_mut(pointer).unwrap() = replacement;
        add("manifest", input);
    }
    let mut duplicate = manifest();
    let mut other = duplicate["offers"][0].clone();
    other["offerId"] = json!("other");
    duplicate["offers"].as_array_mut().unwrap().push(other);
    add("manifest", duplicate);
    let mut excessive = manifest();
    let tool = excessive["offers"][0]["tools"][0].clone();
    excessive["offers"][0]["tools"] = json!(
        (0..8)
            .map(|n| {
                let mut tool = tool.clone();
                tool["name"] = json!(format!("tool-{n}"));
                tool["description"] = json!("x".repeat(8192));
                tool
            })
            .collect::<Vec<_>>()
    );
    add("manifest", excessive);
    // JS expands these numbers while serde_json uses exponent notation. Both
    // the individual schema budget and the complete manifest budget apply.
    for count in [1500, 1600] {
        let mut input = manifest();
        input["offers"][0]["tools"][0]["inputSchema"] =
            json!({"type":"object", "enum":vec![1e20; count]});
        add("manifest", input);
    }
    for count in [1200, 1400] {
        let mut input = manifest();
        let mut tool = input["offers"][0]["tools"][0].clone();
        tool["inputSchema"] = json!({"type":"object", "enum":vec![1e20; count]});
        let mut second = tool.clone();
        second["name"] = json!("other");
        input["offers"][0]["tools"] = json!([tool, second]);
        add("manifest", input);
    }
    for input in [
        json!({"content":[], "structuredContent":null}),
        json!({"content":[{"type":"text","text":""},
            {"type":"image","data":"Zg==","mimeType":"image/svg+xml"},
            {"type":"audio","data":"Zm8=","mimeType":"audio/wav"},
            {"type":"resource","uri":"local:test","text":"","blob":"Zg=="},
            {"type":"resource_link","uri":"https://example.invalid","name":"resource"},
            {"type":"unknown","value":[null,{"x":true}]}]}),
        json!({"content":[{"type":"image","data":"Zh==","mimeType":"image/png"}]}),
        json!({"content":[{"type":"image","data":"Zg==","mimeType":"image/x%"}]}),
        json!({"content":[{"type":"resource","uri":"local:test","mimeType":null}]}),
        json!({"content":[{"type":"unknown","value":{"":false}}]}),
    ] {
        add("call-result", input);
    }
    add(
        "result",
        json!({"registrationId":"registration-1","revision":1.0}),
    );
    add(
        "result",
        json!({"registrationId":"registration-1","revision":9007199254740992_u64}),
    );
    add("unregister", json!({"registrationId":"registration-1"}));
    add(
        "unregister",
        json!({"registrationId":"registration-1","extra":true}),
    );
    for pattern in [
        "(?<=a)(a)\\1",
        "(?<a>a)|(?<a>b)",
        "(?i:a)",
        "[[]",
        "a{184467440737095516160,184467440737095516159}",
        "(",
    ] {
        add("schema", json!({"type":"object","pattern":pattern}));
    }
    for fields in [
        json!({"kind":"accepted", "admissionEvidence":{"kind":"browser_url","url":"not a URL"}}),
        json!({"kind":"accepted", "admissionEvidence":{"kind":"none","extra":true}}),
        json!({"kind":"result_chunk", "index":9007199254740991_u64,"data":"AA=="}),
        json!({"kind":"result_chunk", "index":0,"data":"AB=="}),
        json!({"kind":"result_start", "byteLength":25165824,"chunkCount":683}),
        json!({"kind":"result_start", "byteLength":36865,"chunkCount":1}),
        json!({"kind":"progress", "current":1.0,"total":1.0}),
        json!({"kind":"failed", "message":""}),
        json!({"kind":"rejected", "message":"😀".repeat(2049)}),
        json!({"kind":"result", "result":{"content":[],"structuredContent":null}}),
        json!({"kind":"result", "result":{"content":[],"structuredContent":vec![1e20; 1900]}}),
    ] {
        let mut frame = fields;
        frame["kind"] = json!(format!(
            "client.capability.{}",
            frame["kind"].as_str().unwrap()
        ));
        frame["invocationId"] = json!("call");
        add("client-frame", frame);
    }
    let form_frame = |field: Value| {
        json!({"kind":"client.capability.interaction_request",
        "invocationId":"call", "interactionId":"form", "request":{
            "message":"Complete", "requester":{"name":"Tool", "source":""}, "fields":[field]}})
    };
    for field in [
        json!({"kind":"string","name":"s","label":"Value","required":true,"maxLength":32}),
        json!({"kind":"string","name":"s","label":"Value\n","required":true,"maxLength":32,"default":"x\n"}),
        json!({"kind":"string","name":"s","label":"Value","required":false}),
        json!({"kind":"string","name":"__proto__","label":"Value","required":true,"maxLength":32}),
        json!({"kind":"integer","name":"n","label":"Value","required":true,"minimum":0.1,"maximum":0.9}),
        json!({"kind":"integer","name":"n","label":"Value","required":false,"minimum":0.1,"maximum":0.9}),
        json!({"kind":"number","name":"n","label":"Value","required":true,"minimum":0.2}),
        json!({"kind":"boolean","name":"b","label":"Value","required":true,"default":false}),
        json!({"kind":"single_select","name":"s","label":"Value","required":true,"options":[{"value":"","label":"Empty"}]}),
        json!({"kind":"multi_select","name":"s","label":"Value","required":true,"options":[{"value":"a","label":"A"}],"minItems":1}),
    ] {
        add("client-frame", form_frame(field));
    }
    for (format, default) in [
        ("email", "a@b.co"),
        ("email", "a b@c.co"),
        ("date", "2000-02-29"),
        ("date", "1900-02-29"),
        ("date-time", "0000-01-01T00:00:00+23:59"),
        ("date-time", "2000-01-01T24:00:00Z"),
        ("date-time", "2000-01-01T00:00:00Z\n"),
        ("date", "2000-01-01\n"),
        ("uri", "mailto:x@y.co"),
        ("uri", "relative/path"),
    ] {
        add(
            "client-frame",
            form_frame(json!({"kind":"string","name":"s","label":"Value",
            "required":true,"maxLength":32,"format":format,"default":default})),
        );
    }
    for result in [
        json!({"action":"cancel"}),
        json!({"action":"decline","kind":"form"}),
        json!({"action":"accept","values":{"s":"","n":1.5,"b":false,"a":["a","b"]}}),
        json!({"action":"cancel","values":{}}),
        json!({"action":"accept","values":{"a":["x","x"]}}),
        json!({"action":"accept","values":{"a":null}}),
    ] {
        add(
            "host-frame",
            json!({"kind":"client.capability.interaction_result",
        "invocationId":"call","interactionId":"form","result":result}),
        );
    }
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/support/source.mjs"))
        .arg("crates/protocol/tests/fixtures/capabilities.mjs")
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
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("original-client-capability-contract")
    );
}
