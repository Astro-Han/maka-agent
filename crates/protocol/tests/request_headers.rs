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

use maka_protocol::request_headers::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn header_operations_match_source_normalization_retention_and_closed_results() {
    const ID: &str = "12345678-1234-4234-8234-123456789abc";
    let mut cases = Vec::new();
    for (operation, output, values) in [
        (
            "query",
            false,
            vec![
                json!({"connectionId":ID}),
                json!({"connectionId":"../c"}),
                json!({"connectionId":ID,"extra":1}),
            ],
        ),
        (
            "replace",
            false,
            vec![
                json!({"connectionId":ID,"headers":[]}),
                json!({"connectionId":ID,"headers":[{"name":"\u{feff} X-Keep \u{feff}"},{"name":"X-New","value":"\tÿ"}]}),
                json!({"connectionId":ID,"headers":[{"name":"X-A","value":null}]}),
                json!({"connectionId":ID,"headers":[{"name":"X-A","extra":1}]}),
                json!({"connectionId":ID,"headers":[{"name":"X-A"},{"name":"x-a"}]}),
                json!({"connectionId":ID,"headers":[{"name":"Authorization","value":"token"}]}),
                json!({"connectionId":ID,"headers":[{"name":"\u{85}X-A","value":"ok"}]}),
                json!({"connectionId":ID,"headers":[{"name":"X-A","value":"bad\r\nheader"}]}),
                json!({"connectionId":ID,"headers":[{"name":"X-A","value":"😀"}]}),
                json!({"connectionId":ID,"headers":[{"name":"X-A","value":""}]}),
                json!({"connectionId":ID,"headers":[{"name":"X-A","value":"x".repeat(8192)}]}),
                json!({"connectionId":ID,"headers":[{"name":"X-A","value":"x".repeat(8193)}]}),
                json!({"connectionId":ID,"headers":(0..33).map(|i|json!({"name":format!("X-{i}")})).collect::<Vec<_>>()}),
            ],
        ),
        (
            "query",
            true,
            vec![
                json!({"kind":"found","names":["\u{feff} X-Name "]}),
                json!({"kind":"connection_not_found"}),
                json!({"kind":"found","names":["X-A","x-a"]}),
                json!({"kind":"found","names":["x-api-key"]}),
                json!({"kind":"found"}),
                json!({"kind":"found","names":[],"values":{}}),
                json!({"kind":"connection_not_found","names":[]}),
            ],
        ),
        (
            "replace",
            true,
            vec![
                json!({"kind":"committed","names":["X-A"]}),
                json!({"kind":"unchanged","names":[]}),
                json!({"kind":"connection_not_found"}),
                json!({"kind":"found","names":[]}),
                json!({"kind":"committed","names":[null]}),
            ],
        ),
    ] {
        for value in values {
            let decoded: Option<Value> = match (operation, output) {
                ("query", false) => decode_query(&value).ok().map(|v| json!(v)),
                ("replace", false) => decode_replace(&value).ok().map(|v| json!(v)),
                ("query", true) => decode_query_result(&value).ok().map(|v| json!(v)),
                _ => decode_replace_result(&value).ok().map(|v| json!(v)),
            };
            cases.push(
                json!({"operation":operation,"output":output,"value":value,"expected":decoded}),
            );
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new("node")
        .arg(root.join("crates/protocol/tests/support/request_headers_source.mjs"))
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
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
