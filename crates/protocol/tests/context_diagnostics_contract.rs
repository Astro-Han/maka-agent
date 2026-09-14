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
fn diagnostics_shapes_and_bounds_agree_with_original_source() {
    let mut cases = Vec::new();
    let mut add = |direction: &str, value: Value, valid: bool| {
        let result = if direction == "input" {
            decode_context_diagnostics_input(&value).map(|v| serde_json::to_value(v).unwrap())
        } else {
            decode_context_diagnostics_result(&value).map(|v| serde_json::to_value(v).unwrap())
        };
        assert_eq!(result.is_ok(), valid, "{direction}: {value}: {result:?}");
        let expected = match result {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(_) => json!({"ok":false}),
        };
        cases.push(
            json!({"operation":"context.diagnostics.query","direction":direction,
            "value":value,"expected":expected}),
        );
    };
    add("input", json!({"sessionId":"session"}), true);
    for value in [
        json!({}),
        json!({"sessionId":"bad.id"}),
        json!({"sessionId":"session","turnId":"turn"}),
        json!({"sessionId":null}),
    ] {
        add("input", value, false);
    }
    for reason in ["no_completed_request", "trace_unavailable", "unknown", ""] {
        add(
            "output",
            json!({"status":"unavailable","reason":reason}),
            matches!(reason, "no_completed_request" | "trace_unavailable"),
        );
    }
    add(
        "output",
        json!({"status":"unavailable","reason":"no_completed_request","inputTokens":0}),
        false,
    );
    let available =
        json!({"status":"available","providerId":"openai","modelId":"model","completedAt":1.0});
    add("output", available.clone(), true);
    for field in ["providerId", "modelId", "completedAt"] {
        let mut missing = available.clone();
        missing.as_object_mut().unwrap().remove(field);
        add("output", missing, false);
    }
    for field in [
        "inputTokens",
        "cacheReadInputTokens",
        "contextWindow",
        "completedAt",
    ] {
        for (number, valid) in [
            (json!(0), field != "contextWindow"),
            (json!(1.0), true),
            (json!(9_007_199_254_740_991u64), true),
            (json!(9_007_199_254_740_992u64), false),
            (json!(0.5), false),
            (json!(-1), false),
            (Value::Null, false),
        ] {
            let mut value = available.clone();
            value[field] = number;
            add("output", value, valid);
        }
    }
    for field in ["providerId", "modelId"] {
        for (text, valid) in [
            ("".into(), false),
            ("😀".repeat(256), true),
            ("😀".repeat(257), false),
        ] {
            let mut value = available.clone();
            value[field] = json!(text);
            add("output", value, valid);
        }
    }
    let full = json!({"status":"available","providerId":"openai","modelId":"model","completedAt":1,
        "inputTokens":0,"cacheReadInputTokens":1,"contextWindow":10,
        "composition":{"segments":[{"kind":"messages","bytes":0}],
            "tools":[{"name":"tool","bytes":1.0}],"remainingTools":{"count":0,"bytes":0},"unlabelledToolBytes":0},
        "compaction":{"kind":"history","phase":"pre_turn","eventCount":0,"turnCount":0,"estimatedTokens":0}});
    // Wire accepts zero bytes, empty arrays and unmatched cache/tool totals;
    // producer evidence validation must not silently tighten this codec.
    add("output", full.clone(), true);
    for (pointer, value, valid) in [
        ("/composition/segments", json!([]), true),
        (
            "/composition/segments",
            json!(vec![json!({"kind":"other","bytes":1}); 4]),
            true,
        ),
        (
            "/composition/segments",
            json!(vec![json!({"kind":"other","bytes":1}); 5]),
            false,
        ),
        (
            "/composition/tools",
            json!(vec![json!({"name":"x","bytes":0}); 256]),
            true,
        ),
        (
            "/composition/tools",
            json!(vec![json!({"name":"x","bytes":0}); 257]),
            false,
        ),
        ("/composition/tools/0/name", json!("😀".repeat(257)), false),
        ("/composition/segments/0/kind", json!("invalid"), false),
        ("/composition/segments/0/bytes", json!(0.5), false),
        ("/composition/remainingTools/count", json!(-1), false),
        ("/composition/unlabelledToolBytes", Value::Null, false),
        ("/compaction/phase", json!("mid_turn"), true),
        ("/compaction/phase", json!("standalone"), false),
        ("/compaction/kind", json!("prune"), false),
        (
            "/compaction/eventCount",
            json!(9_007_199_254_740_992u64),
            false,
        ),
        ("/composition", Value::Null, false),
        ("/compaction", Value::Null, false),
    ] {
        let mut changed = full.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        add("output", changed, valid);
    }
    for pointer in [
        "",
        "/composition",
        "/composition/tools/0",
        "/composition/segments/0",
        "/composition/remainingTools",
        "/compaction",
    ] {
        let mut extra = full.clone();
        extra
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), json!(1));
        add("output", extra, false);
    }

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
