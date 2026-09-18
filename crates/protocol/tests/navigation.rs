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

use maka_protocol::{Operation, navigation};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn current_source_navigation_codec_preserves_nullable_fields_and_bounded_state() {
    let state = json!({"type":"turn_state","id":"","turnId":"","ts":-1.5,"status":"running",
        "parentTurnId":"","retry":{"decision":"exhausted","attempts":1.0},"partialOutputRetained":"retired"});
    let contribution = json!({"turnId":"turn","firstSequence":1.0,"latestState":{"sequence":2.0,"message":state},"userPromptPreview":null});
    let turns = json!({"sessionId":"session","throughSequence":3.0,"contributions":[contribution],"nextPosition":null});
    let landmarks = json!({"sessionId":"session","throughSequence":null,"landmarks":[{"turnId":"turn","sequence":1.0,"lastSequence":2.0,"label":"😀".repeat(24)}]});
    let input = json!({"sessionId":"session","throughSequence":null,"position":0.0,"maxContributions":128.0});
    let mut cases = vec![
        (Operation::SessionTurnsQuery, "input", input.clone()),
        (Operation::SessionTurnsQuery, "output", turns.clone()),
        (
            Operation::SessionTurnLandmarksQuery,
            "output",
            landmarks.clone(),
        ),
        (
            Operation::SessionTurnLandmarksQuery,
            "input",
            json!({"sessionId":"session","maxLandmarks":1.0,"turnId":null}),
        ),
    ];
    for (field, value) in [
        ("sessionId", json!("中文")),
        ("maxContributions", json!(0)),
        ("position", json!(9007199254740992u64)),
        ("extra", json!(true)),
    ] {
        let mut bad = input.clone();
        bad[field] = value;
        cases.push((Operation::SessionTurnsQuery, "input", bad));
    }
    let mut missing = input.clone();
    missing.as_object_mut().unwrap().remove("throughSequence");
    cases.push((Operation::SessionTurnsQuery, "input", missing));
    for (field, value) in [
        ("abortSource", Value::Null),
        ("abortSource", json!("😀".repeat(33))),
        ("errorClass", json!(true)),
        ("status", json!("cancelled")),
        ("failureMessage", json!("a".repeat(2049))),
        ("retry", json!({"decision":"exhausted","attempts":0})),
        ("retry", Value::Null),
        ("extra", json!(1)),
    ] {
        let mut bad = turns.clone();
        bad["contributions"][0]["latestState"]["message"][field] = value;
        cases.push((Operation::SessionTurnsQuery, "output", bad));
    }
    let mut bad = landmarks.clone();
    bad["landmarks"][0]["label"] = json!("😀".repeat(25));
    cases.push((Operation::SessionTurnLandmarksQuery, "output", bad));
    let mut bad = turns.clone();
    bad["contributions"][0]["userPromptPreview"] = json!("a".repeat(257));
    cases.push((Operation::SessionTurnsQuery, "output", bad));
    let mut missing = turns.clone();
    missing.as_object_mut().unwrap().remove("nextPosition");
    cases.push((Operation::SessionTurnsQuery, "output", missing));

    let expected: Vec<Value> = cases
        .iter()
        .map(|(operation, direction, value)| {
            let result = if *direction == "input" {
                navigation::decode_input(*operation, value)
            } else {
                navigation::decode_output(*operation, value)
            };
            match result {
                Ok(value) => json!({"ok":true,"value":value}),
                Err(_) => json!({"ok":false}),
            }
        })
        .collect();
    assert!(expected[0]["ok"].as_bool().unwrap());
    assert!(expected[1]["ok"].as_bool().unwrap());
    assert!(expected[2]["ok"].as_bool().unwrap());
    let input:Vec<Value>=cases.iter().map(|(operation,direction,value)|json!({"operation":operation,"direction":direction,"value":value})).collect();
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/support/source.mjs"))
        .arg("crates/protocol/tests/fixtures/navigation.mjs")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&input).unwrap())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).unwrap();
    let actual: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(actual, json!(expected));
}
