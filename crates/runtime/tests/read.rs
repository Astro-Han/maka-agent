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

use maka_runtime::read::{MAX_PAGE_CHARS, ReadInput};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn read_pages_preserve_the_source_contract_and_accept_continuations_in_both_directions() {
    let mut cases = Vec::new();
    for text in [
        String::new(),
        "\n\n\n".into(),
        "零\r\n一\n二\n".into(),
        "中文😀\"\\\t".repeat(2_000),
        (0..800)
            .map(|i| format!("line {i}: {}\n", "😀abc中".repeat(i % 20)))
            .collect(),
    ] {
        for input in [
            json!({"path":"报告.txt"}),
            json!({"path":"C:\\work\\source.rs","offset":1,"limit":2}),
            json!({"path":"archive:event_1","offset":2,"limit":300}),
            json!({"path":"maka://runtime/attachments/item","offset":9999}),
        ] {
            for budget in [512, MAX_PAGE_CHARS] {
                cases.push(json!({"text":text,"input":input,"budget":budget}));
            }
        }
    }
    let mut pages = Vec::new();
    for case in &cases {
        let mut input: ReadInput = serde_json::from_value(case["input"].clone()).unwrap();
        loop {
            let page = input
                .resolve()
                .unwrap()
                .page_with_budget(
                    case["text"].as_str().unwrap(),
                    case["budget"].as_u64().unwrap() as usize,
                )
                .unwrap();
            pages.push(
                json!({"text":case["text"],"input":input,"budget":case["budget"],"actual":page}),
            );
            let Some(next) = page.next else { break };
            input = next;
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new("node")
        .arg(root.join("crates/runtime/tests/support/read_page.mjs"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&pages).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(results.len(), pages.len());
    for (case, result) in pages.iter().zip(results) {
        if result["next"].is_null() {
            continue;
        }
        let input: ReadInput = serde_json::from_value(result["next"].clone()).unwrap();
        let remainder = input
            .resolve()
            .unwrap()
            .page_with_budget(case["text"].as_str().unwrap(), usize::MAX)
            .unwrap();
        assert_eq!(
            serde_json::to_value(remainder).unwrap(),
            result["remainder"]
        );
    }
}
