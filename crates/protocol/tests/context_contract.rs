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

use maka_protocol::context::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn compact_codec_agrees_with_original_typescript() {
    let input = json!({"sessionId":"session","turnId":"turn"});
    let live = json!({"sessionId":"session","turnId":"turn","runId":"run",
        "status":"running","rootExecutionKind":"context_compact"});
    let mut cases = Vec::new();
    let mut add = |direction: &str, value: Value, request: Option<Value>| {
        let result = if direction == "input" {
            decode_context_compact_input(&value).map(|v| serde_json::to_value(v).unwrap())
        } else {
            decode_context_compact_result(&value).and_then(|result| {
                if let Some(input) = &request {
                    assert_compact_output_for_input(
                        &decode_context_compact_input(input)?,
                        &result,
                    )?;
                }
                Ok(serde_json::to_value(result).unwrap())
            })
        };
        let expected = match result {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(_) => json!({"ok":false}),
        };
        let mut case = json!({"direction":direction,"value":value,"expected":expected});
        if let Some(input) = request {
            case["input"] = input;
        }
        cases.push(case);
    };
    for value in [
        input.clone(),
        json!({"sessionId":"session"}),
        json!({"sessionId":"session","turnId":"turn","force":true}),
        json!({"sessionId":"","turnId":"turn"}),
        json!({"sessionId":"session","turnId":"bad.id"}),
        json!({"sessionId":"session","turnId":"a".repeat(129)}),
        json!({"sessionId":"session","turnId":null}),
    ] {
        add("input", value, None);
    }
    add(
        "output",
        json!({"kind":"started","turn":live}),
        Some(input.clone()),
    );
    let outcomes = [
        json!({"kind":"compacted","checkpointId":"checkpoint"}),
        json!({"kind":"unchanged","reason":"😀".repeat(128)}),
        json!({"kind":"failed","reason":"provider_failed"}),
    ];
    for outcome in &outcomes {
        let terminal = json!({"sessionId":"session","turnId":"turn","runId":"run",
            "status":"completed","terminalEventId":"terminal:event","contextCompactionOutcome":outcome});
        add(
            "output",
            json!({"kind":"finished","turn":terminal,"outcome":outcome}),
            Some(input.clone()),
        );
    }
    for terminal in [
        json!({"sessionId":"session","turnId":"turn","runId":"run",
            "status":"failed","terminalEventId":"terminal","failureClass":"provider_error"}),
        json!({"sessionId":"session","turnId":"turn","runId":"run",
            "status":"cancelled","terminalEventId":"terminal","abortSource":"user"}),
    ] {
        add(
            "output",
            json!({"kind":"finished","turn":terminal,"outcome":outcomes[2]}),
            Some(input.clone()),
        );
    }
    // The source codec validates shape and identity; it does not impose an extra
    // relationship between result kind and the Turn's lifecycle status.
    add(
        "output",
        json!({"kind":"finished","turn":live,"outcome":outcomes[0]}),
        None,
    );
    for outcome in [
        json!(null),
        json!({"kind":"compacted","checkpointId":"bad.id"}),
        json!({"kind":"unchanged","reason":""}),
        json!({"kind":"unchanged","reason":"😀".repeat(129)}),
        json!({"kind":"failed","reason":"failure","extra":true}),
        json!({"kind":"unknown","reason":"failure"}),
    ] {
        add(
            "output",
            json!({"kind":"finished","turn":live,"outcome":outcome}),
            None,
        );
    }
    for value in [
        json!({"kind":"started","turn":live,"outcome":null}),
        json!({"kind":"started","turn":live,"outcome":outcomes[0]}),
        json!({"kind":"finished","turn":live}),
        json!({"kind":"started","turn":live,"extra":true}),
        json!({"kind":"unknown","turn":live}),
    ] {
        add("output", value, None);
    }
    for field in ["sessionId", "turnId"] {
        let mut foreign = live.clone();
        foreign[field] = json!("foreign");
        for result in [
            json!({"kind":"started","turn":foreign}),
            json!({"kind":"finished","turn":foreign,"outcome":outcomes[0]}),
        ] {
            add("output", result, Some(input.clone()));
        }
    }
    let mut retry = live.clone();
    retry["providerRetry"] = json!({"phase":"scheduled","attempt":1.0,"maxAttempts":2.0,
        "delayMs":0.0,"reason":"timeout"});
    add("output", json!({"kind":"started","turn":retry}), None);
    retry["providerRetry"]["attempt"] = json!(3);
    add("output", json!({"kind":"started","turn":retry}), None);

    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/context_source.mjs"))
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
