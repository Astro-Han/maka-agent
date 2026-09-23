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

use maka_protocol::transcript::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn search_contract_matches_typescript_bounds_order_and_request_correlation() {
    let request = json!({"subscriptionId":"sub","throughSequence":1024,"query":"中文","includeInternal":false,"cursor":null,"maxMatches":2.0});
    let response = json!({"sessionId":"session","throughSequence":1024.0,"matches":[{"sequence":256.0,"preview":"中文🦀"}],"nextCursor":"next"});
    let mut cases = vec![];
    let mut add = |direction: &str, value: Value, input: Option<Value>, valid: bool| {
        let result = if direction == "input" {
            decode_transcript_search_input(&value).map(|value| json!(value))
        } else {
            decode_transcript_search_result(&value).and_then(|result| {
                if let Some(input) = &input {
                    validate_search_result(
                        &decode_transcript_search_input(input)?,
                        &result,
                        "session",
                    )?;
                }
                Ok(json!(result))
            })
        };
        assert_eq!(result.is_ok(), valid, "{direction} {value}: {result:?}");
        let expected = match result {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(_) => json!({"ok":false}),
        };
        cases.push(json!({"direction":direction,"value":value,"input":input,"expected":expected}));
    };
    add("input", request.clone(), None, true);
    add("output", response.clone(), Some(request.clone()), true);
    for (field, value) in [
        ("query", json!("界".repeat(171))),
        ("query", json!("")),
        ("maxMatches", json!(65)),
        ("maxMatches", json!(true)),
        ("maxMatches", json!(0)),
        ("includeInternal", json!(null)),
        ("throughSequence", json!(9007199254740992u64)),
        ("cursor", json!("x".repeat(1025))),
        ("extra", json!(1)),
    ] {
        let mut v = request.clone();
        v[field] = value;
        add("input", v, None, false);
    }
    for field in ["cursor", "throughSequence", "includeInternal"] {
        let mut v = request.clone();
        v.as_object_mut().unwrap().remove(field);
        add("input", v, None, false);
    }
    for (path, value) in [
        ("/matches/0/sequence", json!(1025)),
        ("/matches/0/preview", json!("界".repeat(129))),
        ("/matches/0/preview", json!("")),
        ("/sessionId", json!("bad.id")),
        ("/nextCursor", json!("")),
        ("/throughSequence", Value::Null),
        (
            "/matches",
            json!([{"sequence":256,"preview":"a"},{"sequence":256,"preview":"b"}]),
        ),
    ] {
        let mut v = response.clone();
        *v.pointer_mut(path).unwrap() = value;
        add("output", v, None, false);
    }
    let mut empty = response.clone();
    empty["matches"] = json!([]);
    empty["throughSequence"] = Value::Null;
    empty["nextCursor"] = Value::Null;
    add("output", empty.clone(), None, true);
    empty["nextCursor"] = json!("unreachable");
    add("output", empty, None, false);
    let mut changed = request.clone();
    changed["throughSequence"] = json!(1023);
    add("output", response.clone(), Some(changed), false);
    let mut changed = request.clone();
    changed["cursor"] = json!("next");
    add("output", response.clone(), Some(changed), false);
    let mut repeated = response.clone();
    repeated["matches"] = json!([{"sequence":1,"preview":"a"},{"sequence":2,"preview":"b"},{"sequence":3,"preview":"c"}]);
    add("output", repeated, Some(request), false);
    let mut child = Command::new("node")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/support/transcript_search_source.mjs"),
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
}
