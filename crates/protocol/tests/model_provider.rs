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

use maka_protocol::model_provider::{assert_page, decode_page, decode_query};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn provider_directory_binds_pages_to_revision_scope_and_utf8_cursor_in_both_clients() {
    let entry = |name: &str, scope: &str| {
        json!({
            "identity":{"packageId":"external.providers","entryId":"providers","scope":scope,"name":name},
            "descriptor":{"label":"Provider","configurationSchema":{},"configurationDefaults":{},
                "authentication":[],"anonymous":true,"discovery":true}
        })
    };
    let page = |entries: Vec<Value>, next: Value| {
        json!({
            "kind":"page","revision":7,"entries":entries,"next":next
        })
    };
    let profile = page(vec![entry("a", "profile")], Value::Null);
    let session = page(
        vec![entry("a", "profile"), entry("b", "session:mine")],
        json!("b"),
    );
    let unicode = page(
        vec![entry("\u{e000}", "profile"), entry("😀", "profile")],
        json!("😀"),
    );
    let mut cases = vec![
        (json!({}), profile.clone(), true),
        (
            json!({"scope":"session:mine","revision":7}),
            session.clone(),
            true,
        ),
        (json!({"scope":"session:other"}), session.clone(), false),
        (json!({"scope":"profile"}), session.clone(), false),
        (json!({"scope":"desktop-ui"}), profile.clone(), false),
        (json!({"revision":8}), profile.clone(), false),
        (json!({"after":"a"}), profile.clone(), false),
        (json!({"after":"a","revision":7}), profile.clone(), false),
        (json!({"after":"0","revision":7}), profile.clone(), true),
        (json!({}), unicode.clone(), true),
        (
            json!({"revision":7,"after":"\u{e000}"}),
            page(vec![entry("😀", "profile")], Value::Null),
            true,
        ),
        (
            json!({}),
            page(
                vec![entry("😀", "profile"), entry("\u{e000}", "profile")],
                Value::Null,
            ),
            false,
        ),
        (
            json!({}),
            page(
                vec![entry("a", "profile"), entry("a", "profile")],
                Value::Null,
            ),
            false,
        ),
        (json!({}), page(vec![], json!("a")), false),
        (
            json!({}),
            page(vec![entry("a", "profile")], json!("b")),
            false,
        ),
        (
            json!({"revision":6}),
            json!({"kind":"revision_changed","revision":7}),
            true,
        ),
        (
            json!({"revision":7}),
            json!({"kind":"revision_changed","revision":7}),
            false,
        ),
        (
            json!({}),
            json!({"kind":"revision_changed","revision":7}),
            false,
        ),
    ];
    let mut changed_identity = profile.clone();
    changed_identity["entries"][0]["identity"]["scope"] = json!("desktop-ui");
    cases.push((json!({}), changed_identity, false));
    let mut leaked_secret = profile;
    leaked_secret["entries"][0]["credential"] = json!("private");
    cases.push((json!({}), leaked_secret, false));
    let values: Vec<_> = cases
        .into_iter()
        .map(|(input, output, expected)| {
            let actual = decode_query(&input)
                .and_then(|query| {
                    let page = decode_page(&output)?;
                    assert_page(&query, &page)
                })
                .is_ok();
            assert_eq!(actual, expected, "{input} => {output}");
            json!({"input":input,"output":output,"expected":expected})
        })
        .collect();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut child = Command::new("node")
        .arg(root.join("crates/protocol/tests/support/provider_source.mjs"))
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
        .write_all(&serde_json::to_vec(&values).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
