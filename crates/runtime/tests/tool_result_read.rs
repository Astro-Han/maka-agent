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

use maka_runtime::{archive::ToolResultAddress, read::ReadInput};
use serde_json::{Value, json};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn page(tool: &str, body: &str, input: &ReadInput) -> Value {
    serde_json::to_value(
        input
            .resolve()
            .unwrap()
            .tool_result_page(tool, body)
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn verified_read_envelopes_preserve_lines_and_opaque_results_preserve_continuations() {
    let text = (0..400)
        .map(|i| format!("line {i}: {}", "source text ".repeat(5)))
        .collect::<Vec<_>>()
        .join("\n");
    let path = ToolResultAddress::event_path("event/中文").unwrap();
    let input: ReadInput =
        serde_json::from_value(json!({"path":path,"offset":3,"limit":5})).unwrap();
    let mut cases = Vec::new();
    for body in [
        json!({"content":text}).to_string(),
        json!(text).to_string(),
        json!({"kind":"text","text":text}).to_string(),
    ] {
        let actual = page("Read", &body, &input);
        assert_eq!(
            actual["content"],
            text.lines().skip(3).take(5).collect::<Vec<_>>().join("\n")
        );
        assert_eq!(actual["totalLines"], 400);
        assert_eq!(actual["returnedLines"], 5);
        assert_eq!(actual["next"], Value::Null, "explicit range is complete");
        cases.push(json!({"body":body,"input":input,"actual":actual}));
    }
    let source_page = input.resolve().unwrap().page(&text).unwrap();
    let terminal = json!({"kind":"terminal","cwd":"/work","cmd":"fixture","status":"completed",
        "exitCode":0,"output":{"mode":"pipes","stdout":text,"stderr":"",
        "stdoutTruncated":true,"stderrTruncated":false,"redacted":false}})
    .to_string();
    let actual = page("Bash", &terminal, &input);
    assert_eq!(actual["metadata"]["stdoutTruncated"], true);
    assert_eq!(actual["metadata"]["exitCode"], 0);
    assert_eq!(actual["metadata"]["kind"], "terminal");
    cases.push(json!({"body":terminal,"input":input,"actual":actual}));
    let long_input: ReadInput = serde_json::from_value(json!({"path":"source.txt"})).unwrap();
    let paged = long_input
        .resolve()
        .unwrap()
        .page(&"😀".repeat(6000))
        .unwrap();
    assert!(paged.next.is_some());
    // Do not silently unwrap arbitrary tools' content fields or lose a saved page's next.
    for (tool, body) in [
        ("CustomTool", json!({"content":text}).to_string()),
        ("Read", serde_json::to_string(&source_page).unwrap()),
        ("Read", serde_json::to_string(&paged).unwrap()),
    ] {
        let first: ReadInput = serde_json::from_value(json!({"path":path})).unwrap();
        let mut current = first;
        let mut restored = String::new();
        loop {
            let result = current
                .resolve()
                .unwrap()
                .tool_result_page(tool, &body)
                .unwrap();
            assert!(
                serde_json::to_string(&result)
                    .unwrap()
                    .encode_utf16()
                    .count()
                    <= 7500
            );
            restored.push_str(&result.content);
            let Some(next) = result.next else { break };
            current = next;
        }
        assert_eq!(restored, body);
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new("node")
        .arg(root.join("crates/runtime/tests/support/tool_result_read.mjs"))
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
