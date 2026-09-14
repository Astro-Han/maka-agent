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

use maka_protocol::execution_boundary::{decode_input, decode_output};
use serde_json::json;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn boundary_projection_matches_source_closed_variants_identity_and_revision_limits() {
    let mut cases = Vec::new();
    for value in [
        json!({"sessionId":"s_1-2"}),
        json!({"sessionId":""}),
        json!({"sessionId":"a".repeat(129)}),
        json!({"sessionId":"a/b"}),
        json!({"sessionId":"s","extra":true}),
        json!({}),
    ] {
        let result = decode_input(&value).map(|id| json!({"sessionId":id}));
        cases.push(json!({"output":false,"value":value,"expected":result.ok()}));
    }
    for value in [
        json!({"kind":"managed","access":"read_only","revision":0}),
        json!({"kind":"managed","access":"writable","revision":2}),
        json!({"kind":"bypass","revision":1.0}),
        json!({"kind":"external","revision":9007199254740991u64}),
        json!({"kind":"managed","access":"write","revision":0}),
        json!({"kind":"managed","revision":0}),
        json!({"kind":"bypass","access":"writable","revision":0}),
        json!({"kind":"bypass","revision":-1}),
        json!({"kind":"bypass","revision":0.5}),
        json!({"kind":"bypass","revision":9007199254740992u64}),
        json!({"kind":"bypass","revision":"1"}),
        json!({"kind":"external"}),
        json!({"kind":"unknown","revision":0}),
    ] {
        let result = decode_output(&value).map(|v| json!(v));
        cases.push(json!({"output":true,"value":value,"expected":result.ok()}));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new("node")
        .arg(root.join("crates/protocol/tests/support/execution_boundary_source.mjs"))
        .current_dir(&root)
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
