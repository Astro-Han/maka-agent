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

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use maka_protocol::Operation;
use serde_json::{Map, Value, json};

#[test]
fn operation_inventory_and_execution_targets_match_current_typescript() {
    let mut operations = Map::new();
    for &operation in Operation::ALL {
        let name = operation.as_str();
        assert_eq!(name.parse::<Operation>().unwrap(), operation);
        assert_eq!(serde_json::to_value(operation).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<Operation>(json!(name)).unwrap(),
            operation
        );
        assert!(
            operations
                .insert(
                    name.to_owned(),
                    json!({"mode": operation.mode(), "availability": operation.availability(), "unavailableError": operation.unavailable_error()}),
                )
                .is_none(),
            "Duplicate operation: {name}"
        );
    }

    // The existing probe bundles current worktree sources and rejects workspace dist.
    // MAKA_JS_DEPS may point to a separate checkout providing npm dependencies only.
    let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/support/source.mjs");
    let mut child = Command::new("node")
        .arg(probe)
        .arg("crates/protocol/tests/fixtures/operations.mjs")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Operation source contract requires Node and root npm dependencies");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            &serde_json::to_vec(&json!({
                "epoch": maka_protocol::COMPATIBILITY_EPOCH,
                "operations": operations,
                "targets": execution_targets(),
            }))
            .unwrap(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(report["check"], "operation-contract");
    assert_eq!(report["operationCount"], Operation::ALL.len());
}

fn execution_targets() -> Vec<Value> {
    let model =
        json!({"llmConnectionId":"connection", "llmConnectionSlug":"fixture", "model":"fixture"});
    let mut cases = Vec::new();
    for target in [
        json!({"modelTarget":{"kind":"default"}}),
        json!({"executorId":"Agent.v1:fixture"}),
        json!({"executorId":"a".repeat(128)}),
        json!({"executorId":"a".repeat(129)}),
        json!({"executorId":"1agent"}),
        json!({"executorId":null}),
        json!({"executorId":"Agent", "modelTarget":{"kind":"default"}}),
        json!({"executorId":"Agent", "unexpected":true}),
        json!({}),
    ] {
        let mut input =
            json!({"sessionId":"session", "workspace":{"kind":"host_path", "path":"/work"}});
        input
            .as_object_mut()
            .unwrap()
            .extend(target.as_object().unwrap().clone());
        let decoded = maka_protocol::session::decode_session_create_input(&input)
            .ok()
            .map(|value| serde_json::to_value(value).unwrap());
        cases.push(json!({"operation":Operation::SessionCreate, "input":input, "decoded":decoded}));
    }
    for defaults in [
        json!({}),
        json!({"model":model}),
        json!({"executorId":"Agent.v1:fixture", "permissionMode":"bypass"}),
        json!({"executorId":"Agent", "model":model}),
        json!({"executorId":"Agent/invalid"}),
        json!({"executorId":null}),
        json!({"model":null}),
        json!({"executorId":"Agent", "unexpected":true}),
    ] {
        let input = json!({"turnId":"turn", "actionId":"action", "proposal":{"disposition":"create_new", "title":"Task"},
            "create":{"workspace":{"kind":"host_path", "path":"/work"}}, "newWorkDefaults":defaults});
        let decoded = maka_protocol::workhub::decode_act(&input)
            .ok()
            .map(|value| serde_json::to_value(value).unwrap());
        cases.push(json!({"operation":Operation::WorkhubCoordinationActFromTurn, "input":input, "decoded":decoded}));
    }
    cases
}
