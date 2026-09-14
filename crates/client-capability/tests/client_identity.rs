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

use maka_client_capability::{Identity, PrincipalKind, proxy_tool_name};
use serde_json::json;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn provider_and_model_names_match_the_original_source() {
    let mut cases = Vec::new();
    for principal_kind in [
        PrincipalKind::LocalOwner,
        PrincipalKind::RemoteOwner,
        PrincipalKind::CapabilityProvider,
    ] {
        let identity = Identity {
            principal_kind,
            principal_id: "owner\n界".into(),
            client_instance_id: "desktop".into(),
            credential_bound_client_instance_id: None,
            capability_owner: None,
        };
        cases.push(
            json!({"kind":"provider-id", "input":{"principalKind":principal_kind,
            "principalId":identity.principal_id,"clientInstanceId":identity.client_instance_id},
            "expected":{"ok":true,"value":identity.provider_id()}}),
        );
    }
    for server in [
        "desktop",
        "__Ｆｏｏ__",
        "ÅΩ",
        "___",
        "server.with:punctuation",
    ] {
        for tool in [
            "foo.bar",
            "foo_bar",
            "a._?_-z",
            "ﬀ①",
            "⛵",
            &"x".repeat(100),
        ] {
            cases.push(
                json!({"kind":"proxy-name", "input":{"serverId":server,"toolName":tool},
                "expected":{"ok":true,"value":proxy_tool_name(server,tool)}}),
            );
        }
    }
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/support/source.mjs"))
        .arg("crates/protocol/tests/fixtures/capabilities.mjs")
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
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
