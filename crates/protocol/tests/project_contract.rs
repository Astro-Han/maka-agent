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

use maka_protocol::project::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

fn decoded(kind: &str, value: &Value, input: Option<&Value>) -> maka_protocol::Result<Value> {
    Ok(match kind {
        "query_in" => {
            let v = decode_query(value)?;
            json!({"value":v,"hostPaths":v.uses_host_paths()})
        }
        "mutate_in" => {
            let v = decode_mutation(value)?;
            json!({"value":v,"hostPaths":v.uses_host_paths()})
        }
        "query_out" => {
            let result = decode_query_result(value)?;
            if let Some(input) = input {
                assert_query_output(&decode_query(input)?, &result)?;
            }
            json!({"value":result})
        }
        "mutate_out" => json!({"value":decode_mutation_result(value)?}),
        _ => unreachable!(),
    })
}

#[test]
fn project_contract_matches_original_codecs_including_limits_and_path_authority() {
    let revision = format!("sha256:{}", "a".repeat(64));
    let query = json!({"kind":"list_start","view":"locations"});
    let item = json!({"kind":"project","projectIndex":0.0,"id":"project","name":"Repository",
        "aliasCount":1,"locationCount":1,"preferredLocationIndex":0,"archivedAt":null,"available":true});
    let location = json!({"kind":"location","projectIndex":0,"itemIndex":0,
        "location":{"path":"C:\\repo","isWorktree":true}});
    let alias = json!({"kind":"alias","projectIndex":0,"itemIndex":0,"alias":"old"});
    let page = json!({"kind":"page","view":"locations","revision":revision,"projectCount":1.0,
        "items":[item,alias,location],"nextCursor":null});
    let directory = json!({"kind":"directory_page","rootId":"root-1","segments":["repo"],
        "entries":[{"name":"child"}],"nextCursor":null});
    let project = json!({"kind":"project","project":{"id":"project","aliases":["old"],"name":"Repository",
        "locationCount":1,"archivedAt":null,"available":true}});
    let valid = vec![
        ("query_in", query.clone()),
        ("query_in", json!({"kind":"list_start","view":"summary"})),
        (
            "query_in",
            json!({"kind":"list_continue","view":"summary","revision":revision,"cursor":"1"}),
        ),
        ("query_in", json!({"kind":"directory_roots"})),
        (
            "query_in",
            json!({"kind":"directory_list_start","rootId":"root-1","segments":[]}),
        ),
        (
            "query_in",
            json!({"kind":"directory_list_continue","rootId":"root-1","segments":["one"],"cursor":"two"}),
        ),
        (
            "mutate_in",
            json!({"kind":"register","path":"/repo","prefer":false}),
        ),
        ("mutate_in", json!({"kind":"register","path":"C:\\repo"})),
        (
            "mutate_in",
            json!({"kind":"register_directory","rootId":"root-1","segments":["repo"]}),
        ),
        (
            "mutate_in",
            json!({"kind":"relink","projectId":"project","path":"\\\\server\\share\\repo"}),
        ),
        (
            "mutate_in",
            json!({"kind":"rename","projectId":"project","name":"Renamed"}),
        ),
        ("mutate_in", json!({"kind":"archive","projectId":"project"})),
        ("mutate_in", json!({"kind":"restore","projectId":"project"})),
        ("query_out", page.clone()),
        (
            "query_out",
            json!({"kind":"revision_changed","view":"locations","expected":revision,"actual":revision}),
        ),
        ("query_out", directory.clone()),
        (
            "query_out",
            json!({"kind":"directory_roots","roots":[{"id":"root-1","label":"~"}]}),
        ),
        ("mutate_out", project.clone()),
    ];
    let mut cases = Vec::new();
    let mut add = |kind: &str, value: Value, input: Option<Value>| {
        let expected = match decoded(kind, &value, input.as_ref()) {
            Ok(v) => json!({"ok":true,"decoded":v}),
            Err(_) => json!({"ok":false}),
        };
        cases.push(json!({"kind":kind,"value":value,"input":input,"expected":expected}));
    };
    for (kind, value) in &valid {
        assert!(decoded(kind, value, None).is_ok(), "{kind}: {value}");
        add(kind, value.clone(), None);
        for field in value.as_object().unwrap().keys() {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            add(kind, missing, None);
            let mut null = value.clone();
            null[field] = Value::Null;
            add(kind, null, None);
        }
        let mut extra = value.clone();
        extra["unknown"] = json!(true);
        add(kind, extra, None);
    }
    for path in [
        "/",
        "C:/repo",
        "C:repo",
        "relative",
        "\\repo",
        "\\\\server",
        "//server/share",
        "",
    ] {
        add("mutate_in", json!({"kind":"register","path":path}), None);
    }
    for segment in [".", "..", "a/b", "a\\b", "", "é", "\0"] {
        add(
            "query_in",
            json!({"kind":"directory_list_start","rootId":"root-1","segments":[segment]}),
            None,
        );
    }
    for count in [64, 65] {
        add(
            "query_in",
            json!({"kind":"directory_list_start","rootId":"root-1","segments":vec!["a"; count]}),
            None,
        );
    }
    for count in [4096, 4097] {
        add(
            "mutate_in",
            json!({"kind":"rename","projectId":"project","name":"😀".repeat(count)}),
            None,
        );
    }
    for label in [" name", "name ", "\u{feff}name", "\u{0085}name", "name\n"] {
        add(
            "query_out",
            json!({"kind":"directory_roots","roots":[{"id":"root-1","label":label}]}),
            None,
        );
    }
    for (field, value) in [
        ("aliases", json!(["same", "same"])),
        ("id", json!("bad.id")),
        ("locationCount", json!(9007199254740992u64)),
        ("available", json!(1)),
    ] {
        let mut invalid = project.clone();
        invalid["project"][field] = value;
        add("mutate_out", invalid, None);
    }
    let mut missing = project.clone();
    missing["project"]
        .as_object_mut()
        .unwrap()
        .remove("archivedAt");
    add("mutate_out", missing, None);
    for count in [64, 65] {
        let mut boundary = page.clone();
        boundary["items"] = json!(vec![item.clone(); count]);
        add("query_out", boundary, None);
    }
    let mut oversize = page.clone();
    let mut large_item = item.clone();
    large_item["name"] = json!("x".repeat(16 * 1024));
    oversize["items"] = json!(vec![large_item; 3]);
    add("query_out", oversize, None);
    for count in [128, 129] {
        let mut boundary = directory.clone();
        boundary["entries"] = json!(vec![json!({"name":"x"}); count]);
        add("query_out", boundary, None);
    }
    let mut oversize = directory.clone();
    oversize["entries"] = json!(vec![json!({"name":"x".repeat(255)}); 128]);
    add("query_out", oversize, None);
    let mut summary = page.clone();
    summary["view"] = json!("summary");
    add("query_out", summary, Some(query.clone()));
    add("query_out", page, Some(query));
    add(
        "query_out",
        directory.clone(),
        Some(json!({"kind":"directory_list_start","rootId":"root-2","segments":[]})),
    );
    add(
        "query_out",
        directory,
        Some(json!({"kind":"directory_list_start","rootId":"root-1","segments":[]})),
    );
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/project_source.mjs"))
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
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
