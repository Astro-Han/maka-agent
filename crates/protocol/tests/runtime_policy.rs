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

use maka_protocol::runtime_policy::*;
use maka_runtime::configuration::policy::RuntimePolicy;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn policy_wire_matches_current_source_and_domain_normalization() {
    let defaults = serde_json::to_value(RuntimePolicy::default()).unwrap();
    let snapshot = json!({"revision":0,"policy":defaults});
    let mut cases = Vec::new();
    let mut add = |operation: &str, direction: &str, value: Value, valid: bool| {
        let result = match (operation, direction) {
            ("network-proxy.test", "input") => maka_protocol::network_proxy::decode_input(&value)
                .map(|v| serde_json::to_value(v).unwrap()),
            ("network-proxy.test", _) => maka_protocol::network_proxy::decode_output(&value)
                .map(|v| serde_json::to_value(v).unwrap()),
            ("runtime.policy.network-proxy.update", "input") => {
                decode_network_proxy_update(&value).map(|v| serde_json::to_value(v).unwrap())
            }
            ("runtime.policy.network-proxy.update", _) => {
                decode_network_proxy_result(&value).map(|v| serde_json::to_value(v).unwrap())
            }
            ("runtime.policy.query", "input") => decode_query_input(&value).map(|()| json!({})),
            ("runtime.policy.query", _) => {
                decode_query_result(&value).map(|v| serde_json::to_value(v).unwrap())
            }
            (_, "input") => decode_mutation_input(&value).map(|v| serde_json::to_value(v).unwrap()),
            _ => decode_mutation_result(&value).map(|v| serde_json::to_value(v).unwrap()),
        };
        assert_eq!(
            result.is_ok(),
            valid,
            "{operation} {direction}: {value}: {result:?}"
        );
        let expected = match result {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(_) => json!({"ok":false}),
        };
        cases.push(
            json!({"operation":operation,"direction":direction,"value":value,"expected":expected}),
        );
    };
    add("runtime.policy.query", "input", json!({}), true);
    add(
        "runtime.policy.query",
        "input",
        json!({"scope":"root"}),
        false,
    );
    add("runtime.policy.query", "output", snapshot.clone(), true);
    // Parse explicit order: json! maps may sort keys without preserve_order,
    // while workspace feature unification keeps this valid wire permutation.
    let reordered = r#"{"policy":{
        "externalAgents":{"antigravity":{"executable":""}},
        "shell":{"executable":"","preference":"auto"},
        "chatDefaults":{"thinkingLevel":"high","sandboxMode":"workspace-write"},
        "privacy":{"incognitoActive":false},
        "workspaceInstructions":{"enabled":true},
        "memory":{"agentReadEnabled":false,"enabled":true},
        "personalization":{"assistantTone":"","displayName":""},
        "networkProxy":{"username":"","port":7890,"protocol":"http",
            "host":"127.0.0.1","enabled":false,"authEnabled":false,
            "autoBypassDomains":["localhost","127.0.0.1","::1","192.168.*","10.*","*.local"],
            "bypassList":["metaso.cn","baidu.com"]}},"revision":0}"#;
    for revision in ["0", "1.0"] {
        add(
            "runtime.policy.query",
            "output",
            serde_json::from_str(
                &reordered.replace("\"revision\":0", &format!("\"revision\":{revision}")),
            )
            .unwrap(),
            true,
        );
    }
    for (pointer, value, valid) in [
        (
            "/policy/chatDefaults",
            json!({"sandboxMode":"workspace-write","codeModeEnabled":true}),
            false,
        ),
        (
            "/policy/chatDefaults",
            json!({"sandboxMode":"workspace-write","codeModeEnabled":false}),
            false,
        ),
        (
            "/policy/chatDefaults",
            json!({"sandboxMode":"workspace-write","codeModeEnabled":null}),
            false,
        ),
        (
            "/policy/externalAgents/antigravity/executable",
            json!("/Applications/Antigravity.app"),
            true,
        ),
        (
            "/policy/externalAgents/antigravity/executable",
            json!("relative"),
            false,
        ),
        ("/revision", json!(1.0), true),
        ("/revision", json!(9_007_199_254_740_992u64), false),
        (
            "/policy/chatDefaults",
            json!({"sandboxMode":"danger-full-access","thinkingLevel":"high"}),
            true,
        ),
        (
            "/policy/chatDefaults",
            json!({"sandboxMode":"workspace-write","thinkingLevel":null}),
            false,
        ),
        (
            "/policy/chatDefaults",
            json!({"sandboxMode":"read-only"}),
            true,
        ),
        ("/policy/networkProxy/host", json!(" localhost "), false),
        (
            "/policy/personalization/assistantTone",
            json!("😀".repeat(2048)),
            true,
        ),
        (
            "/policy/personalization/assistantTone",
            json!("😀".repeat(2049)),
            false,
        ),
        ("/policy/memory", json!({"enabled":true}), false),
        (
            "/policy/shell",
            json!({"preference":"auto","executable":"","extra":true}),
            false,
        ),
    ] {
        let mut value_copy = snapshot.clone();
        *value_copy.pointer_mut(pointer).unwrap() = value;
        add("runtime.policy.query", "output", value_copy, valid);
    }
    let mut missing = snapshot.clone();
    missing["policy"].as_object_mut().unwrap().remove("privacy");
    add("runtime.policy.query", "output", missing, false);
    let mut oversized = snapshot.clone();
    oversized["policy"]["personalization"]["assistantTone"] = json!("x".repeat(64 * 1024));
    add("runtime.policy.query", "output", oversized, false);
    // Every supported wire kind is decoded even when the Host has no consumer yet.
    for (kind, key) in [
        ("set_network_proxy", "networkProxy"),
        ("set_personalization", "personalization"),
        ("set_memory", "memory"),
        ("set_workspace_instructions", "workspaceInstructions"),
        ("set_privacy", "privacy"),
        ("set_chat_defaults", "chatDefaults"),
        ("set_shell", "shell"),
        ("set_external_agents", "externalAgents"),
    ] {
        add(
            "runtime.policy.mutate",
            "input",
            json!({"expectedRevision":1.0,
            "operation":{"kind":kind,"value":defaults[key]}}),
            true,
        );
    }
    for (kind, value, valid) in [
        (
            "patch_agent_settings",
            json!({"memory":{"enabled":false}}),
            true,
        ),
        ("patch_agent_settings", json!({}), true),
        (
            "set_chat_defaults",
            json!({"sandboxMode":"workspace-write"}),
            true,
        ),
        (
            "set_chat_defaults",
            json!({"sandboxMode":"workspace-write","codeModeEnabled":true}),
            false,
        ),
        (
            "set_chat_defaults",
            json!({"sandboxMode":"workspace-write","codeModeEnabled":false}),
            false,
        ),
        (
            "set_chat_defaults",
            json!({"sandboxMode":"workspace-write","codeModeEnabled":null}),
            false,
        ),
        (
            "set_external_agents",
            json!({"antigravity":{"executable":"/Applications/Antigravity.app"}}),
            true,
        ),
        (
            "set_external_agents",
            json!({"antigravity":{"executable":"C:\\App.exe"}}),
            false,
        ),
        (
            "set_external_agents",
            json!({"antigravity":{"executable":"/bad\npath"}}),
            false,
        ),
        (
            "set_chat_defaults",
            json!({"sandboxMode":"danger-full-access","thinkingLevel":"off"}),
            true,
        ),
        (
            "set_chat_defaults",
            json!({"sandboxMode":"workspace-write","thinkingLevel":null}),
            false,
        ),
        (
            "set_chat_defaults",
            json!({"sandboxMode":"read-only"}),
            true,
        ),
        ("set_chat_defaults", json!({"thinkingLevel":"high"}), false),
        (
            "set_chat_defaults",
            json!({"sandboxMode":"workspace-write","extra":true}),
            false,
        ),
        (
            "set_shell",
            json!({"preference":"git_bash","executable":" /bin/bash "}),
            true,
        ),
        (
            "set_shell",
            json!({"preference":"git_bash","executable":" "}),
            false,
        ),
        ("set_unknown", json!({}), false),
    ] {
        add(
            "runtime.policy.mutate",
            "input",
            json!({"expectedRevision":0,
            "operation":{"kind":kind,"value":value}}),
            valid,
        );
    }
    let mut proxy = defaults["networkProxy"].clone();
    proxy["host"] = json!(" localhost ");
    add(
        "runtime.policy.mutate",
        "input",
        json!({"expectedRevision":0,
        "operation":{"kind":"set_network_proxy","value":proxy}}),
        true,
    );
    for (value, valid) in [
        (json!({"kind":"committed","revision":1.0}), true),
        (
            json!({"kind":"revision_conflict","expectedRevision":0,"actualRevision":2}),
            true,
        ),
        (json!({"kind":"committed","revision":-1}), false),
        (
            json!({"kind":"committed","revision":9_007_199_254_740_992u64}),
            false,
        ),
        (
            json!({"kind":"committed","revision":1,"snapshot":snapshot}),
            false,
        ),
        (
            json!({"kind":"revision_conflict","expectedRevision":0}),
            false,
        ),
        (json!({"kind":"unavailable"}), false),
    ] {
        add("runtime.policy.mutate", "output", value, valid);
    }
    let proxy_op = "runtime.policy.network-proxy.update";
    let mut atomic = json!({"expectedPolicyRevision": 0.0, "expectedCredential":null,
        "networkProxy":defaults["networkProxy"], "credential":{"kind":"delete"}});
    add(proxy_op, "input", atomic.clone(), true);
    for credential in [
        json!({"kind":"delete", "secret":"unexpected"}),
        json!({"kind":"keep"}),
        json!({"kind":"replace", "secret":"test"}),
    ] {
        atomic["credential"] = credential;
        add(proxy_op, "input", atomic.clone(), false);
    }
    atomic["networkProxy"]["authEnabled"] = json!(true);
    atomic["credential"] = json!({"kind":"replace", "secret":"test", "expectedTarget":{
        "protocol":"http", "host":" LOCALHOST ", "port":7890.0,"username":"user"}});
    add(proxy_op, "input", atomic.clone(), true);
    atomic["credential"]["expectedTarget"] = Value::Null;
    add(proxy_op, "input", atomic, false);
    for (value, valid) in [
        (
            json!({"kind":"credential_stale", "expected":null, "actual":null}),
            true,
        ),
        (json!({"kind":"credential_stale", "actual":null}), false),
        (
            json!({"kind":"committed", "revision":1.0,"credentialStatus":{
            "locator":{"scope":"network_proxy","kind":"password"}, "configured":false,
            "credentialId":null, "revision":null, "updatedAt":null}}),
            true,
        ),
        (
            json!({"kind":"proxy_target_mismatch","expected":{
            "protocol":"http","host":" LOCALHOST ","port":7890.0,"username":""},"actual":{
            "protocol":"http","host":"127.0.0.1","port":7890,"username":""}}),
            true,
        ),
    ] {
        add(proxy_op, "output", value, valid);
    }
    for (value, valid) in [
        (json!({}), true),
        (json!({"url":"http://EXAMPLE.com", "timeoutMs":1.0}), true),
        (json!({"url":"file:///tmp/a"}), false),
        (json!({"networkProxy":null}), false),
        (json!({"timeoutMs":null}), false),
        (json!({"timeoutMs":30001}), false),
        (json!({"networkProxy":defaults["networkProxy"]}), true),
        (
            json!({"networkProxy":defaults["networkProxy"],"password":"not a wire field"}),
            false,
        ),
    ] {
        add("network-proxy.test", "input", value, valid);
    }
    for (value, valid) in [
        (
            json!({"ok":true,"latencyMs":1.0,"status":200.0,"ip":"127.0.0.1"}),
            true,
        ),
        (
            json!({"ok":true,"latencyMs":1.0,"status":200.0,"ip":""}),
            false,
        ),
        (json!({"ok":false,"latencyMs":0,"error":"timeout"}), true),
        (json!({"ok":true,"latencyMs":300001}), false),
        (json!({"ok":true,"latencyMs":0,"ip":null}), false),
        (json!({"ok":true,"latencyMs":0,"ip":"😀".repeat(65)}), false),
    ] {
        add("network-proxy.test", "output", value, valid);
    }
    let data = json!({"cases":cases,"defaults":defaults,"errors":{
        "network-proxy.test":maka_protocol::network_proxy::ERRORS,
        "runtime.policy.query":QUERY_ERRORS,"runtime.policy.mutate":MUTATION_ERRORS}});
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/runtime_policy_source.mjs"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let written = child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&data).unwrap());
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    written.unwrap();
}
