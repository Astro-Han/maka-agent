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

use super::*;
use crate::configuration::validation::{revision, text};
use serde_json::Value;
use std::collections::BTreeSet;

pub fn normalize_mutation(mut input: Value) -> Result<RuntimePolicyMutationInput, String> {
    integer_field(&mut input, "expectedRevision");
    if let Some(operation) = input.get_mut("operation")
        && operation.get("kind").and_then(Value::as_str) == Some("set_network_proxy")
        && let Some(value) = operation.get_mut("value")
    {
        integer_field(value, "port");
    }
    if let Some(operation) = input.get_mut("operation")
        && operation.get("kind").and_then(Value::as_str) == Some("set_subagents")
        && let Some(value) = operation.get_mut("value")
    {
        *value = serde_json::to_value(subagents::normalize(value)).map_err(|e| e.to_string())?;
    }
    let mut input: RuntimePolicyMutationInput =
        serde_json::from_value(input).map_err(|e| e.to_string())?;
    revision(input.expected_revision, false)?;
    normalize_operation(&mut input.operation)?;
    Ok(input)
}

pub fn decode_canonical_snapshot(input: Value) -> Result<RuntimePolicySnapshot, String> {
    let encoded = crate::capability::json::stringify(&input).map_err(|e| e.to_string())?;
    if encoded.len() > MAX_POLICY_SNAPSHOT_BYTES {
        return Err("policy snapshot exceeds byte limit".into());
    }
    decode_canonical_document(input)
}

/// Validates persisted policy semantics. The storage owner must bound document
/// bytes before decoding; migrated documents may exceed the wire snapshot budget.
pub fn decode_canonical_document(mut input: Value) -> Result<RuntimePolicySnapshot, String> {
    integer_field(&mut input, "revision");
    if let Some(value) = input
        .get_mut("policy")
        .and_then(|p| p.get_mut("networkProxy"))
    {
        integer_field(value, "port");
    }
    let mut normalized = input.clone();
    if let Some(value) = normalized
        .get_mut("policy")
        .and_then(|p| p.get_mut("subagents"))
    {
        *value = serde_json::to_value(subagents::normalize(value)).map_err(|e| e.to_string())?;
    }
    let mut snapshot: RuntimePolicySnapshot =
        serde_json::from_value(normalized).map_err(|e| e.to_string())?;
    revision(snapshot.revision, false)?;
    normalize_policy(&mut snapshot.policy)?;
    // Object key order is not semantic, including with serde_json/preserve_order.
    // Only integer spellings are normalized in the original comparison value;
    // dropped fields, trimming and subagent normalization must remain detectable.
    if serde_json::to_value(&snapshot).map_err(|e| e.to_string())? != input {
        return Err("runtime policy snapshot must be canonical".into());
    }
    Ok(snapshot)
}

pub(super) fn integer_field(value: &mut Value, key: &str) {
    if let Some(number) = value.get_mut(key)
        && let Some(float) = number.as_f64()
        && float >= 0.0
        && float <= crate::configuration::validation::MAX_SAFE_INTEGER as f64
        && float.fract() == 0.0
    {
        *number = Value::from(float as u64);
    }
}

fn normalize_policy(policy: &mut RuntimePolicy) -> Result<(), String> {
    proxy(&mut policy.network_proxy)?;
    personalization(&policy.personalization)?;
    shell(&mut policy.shell)?;
    external_agents(&policy.external_agents)?;
    Ok(())
}

fn normalize_operation(operation: &mut RuntimePolicyMutation) -> Result<(), String> {
    match operation {
        RuntimePolicyMutation::SetNetworkProxy { value } => proxy(value),
        RuntimePolicyMutation::SetPersonalization { value } => personalization(value),
        RuntimePolicyMutation::SetShell { value } => shell(value),
        RuntimePolicyMutation::SetExternalAgents { value } => external_agents(value),
        RuntimePolicyMutation::PatchAgentSettings { value } => {
            if let Some(value) = &value.personalization {
                if let Some(name) = &value.display_name {
                    text(name, 256, false)?;
                }
                if let Some(tone) = &value.assistant_tone {
                    text(tone, 4096, false)?;
                }
            }
            Ok(())
        }
        RuntimePolicyMutation::SetMemory { .. }
        | RuntimePolicyMutation::SetWorkspaceInstructions { .. }
        | RuntimePolicyMutation::SetPrivacy { .. }
        | RuntimePolicyMutation::SetChatDefaults { .. }
        | RuntimePolicyMutation::SetWebSearch { .. }
        | RuntimePolicyMutation::SetSubagents { .. } => Ok(()),
    }
}
fn personalization(value: &Personalization) -> Result<(), String> {
    text(&value.display_name, 256, false)?;
    text(&value.assistant_tone, 4096, false)
}
fn external_agents(value: &ExternalAgents) -> Result<(), String> {
    let executable = &value.antigravity.executable;
    text(executable, 4096, false)?;
    if !executable.is_empty()
        && (!executable.starts_with('/') || executable.chars().any(|c| c <= '\u{1f}'))
    {
        return Err("Antigravity executable must be an absolute macOS path".into());
    }
    Ok(())
}
fn controls(value: &str) -> bool {
    value
        .chars()
        .any(|c| c <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&c))
}
pub(super) fn trim(value: &str) -> &str {
    // ECMAScript WhiteSpace + LineTerminator; Rust additionally trims U+0085.
    value.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}')
}
pub(super) fn proxy(value: &mut NetworkProxy) -> Result<(), String> {
    text(&value.host, 255, false)?;
    if controls(&value.host) {
        return Err("network proxy host contains controls".into());
    }
    value.host = trim(&value.host).into();
    if value.enabled && value.host.is_empty() {
        return Err("enabled proxy requires host".into());
    }
    if value.port == 0 {
        return Err("invalid proxy port".into());
    }
    text(&value.username, 256, false)?;
    for values in [&value.bypass_list, &value.auto_bypass_domains] {
        if values.len() > 256 {
            return Err("too many proxy domains".into());
        }
        let mut seen = BTreeSet::new();
        for item in values {
            text(item, 512, true)?;
            if !seen.insert(item) {
                return Err("duplicate proxy domain".into());
            }
        }
    }
    Ok(())
}
fn shell(value: &mut ShellPolicy) -> Result<(), String> {
    text(&value.executable, 4096, false)?;
    value.executable = trim(&value.executable).into();
    if controls(&value.executable)
        || (value.preference == ShellPreference::GitBash && value.executable.is_empty())
    {
        return Err("invalid shell executable".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn canonical_policy_rejects_normalizable_values_while_mutations_normalize() {
        let snapshot = RuntimePolicySnapshot {
            revision: 0,
            policy: RuntimePolicy::default(),
        };
        snapshot.validate().unwrap();
        let serialized = serde_json::to_value(&snapshot).unwrap();
        let policy = serialized["policy"].as_object().unwrap();
        let reversed: serde_json::Map<String, Value> = policy
            .iter()
            .rev()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut reordered = json!({"policy":reversed,"revision":0.0});
        reordered["policy"]["networkProxy"]["port"] = json!(7890.0);
        assert_eq!(
            decode_canonical_snapshot(reordered.clone()).unwrap(),
            snapshot
        );
        reordered["policy"]["subagents"]["ignored"] = json!(true);
        assert!(decode_canonical_snapshot(reordered).is_err());
        let mut value = serde_json::to_value(snapshot).unwrap();
        value["policy"]["networkProxy"]["host"] = json!("\u{feff} Example ");
        assert!(decode_canonical_snapshot(value.clone()).is_err());
        let mutation = normalize_mutation(json!({"expectedRevision":0,"operation":{
            "kind":"set_network_proxy","value":value["policy"]["networkProxy"]}}))
        .unwrap();
        let RuntimePolicyMutation::SetNetworkProxy { value } = mutation.operation else {
            panic!()
        };
        assert_eq!(value.host, "Example");
        // Host rejects controls before trimming; shell checks after trimming.
        assert!(
            normalize_mutation(json!({"expectedRevision":0,"operation":{
            "kind":"set_shell","value":{"preference":"auto","executable":"\n x \n"}}}))
            .is_ok()
        );
    }
    #[test]
    fn chat_defaults_are_replacement_and_optional_does_not_mean_nullable() {
        let request = |value| json!({"expectedRevision":0,"operation":{"kind":"set_chat_defaults","value":value}});
        assert!(normalize_mutation(request(json!({"permissionMode":"ask"}))).is_ok());
        assert!(normalize_mutation(request(json!({"permissionMode":"explore"}))).is_err());
        assert!(
            normalize_mutation(request(
                json!({"permissionMode":"ask","thinkingLevel":null})
            ))
            .is_err()
        );
        assert!(normalize_mutation(request(json!({"thinkingLevel":"high"}))).is_err());
        assert!(
            normalize_mutation(json!({"expectedRevision":0,"operation":{
            "kind":"patch_agent_settings","value":{"memory":null}}}))
            .is_err()
        );
    }
}
