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

//! Root control policy. Storing a setting does not grant execution capability.
mod codec;
pub mod network_test;
pub mod network_update;
mod settings;
mod subagents;
use crate::execution::ThinkingLevel;
pub use codec::{decode_canonical_document, decode_canonical_snapshot, normalize_mutation};
use serde::{Deserialize, Serialize};
pub use settings::*;
pub use subagents::{SubagentPreset, SubagentProfile, SubagentSettings};

pub const MAX_POLICY_SNAPSHOT_BYTES: usize = 48 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatDefaultPermissionMode {
    #[default]
    Ask,
    Bypass,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatDefaults {
    pub permission_mode: ChatDefaultPermissionMode,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub code_mode_enabled: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::present"
    )]
    pub thinking_level: Option<ThinkingLevel>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimePolicy {
    pub network_proxy: NetworkProxy,
    pub personalization: Personalization,
    pub memory: MemoryPolicy,
    pub workspace_instructions: EnabledPolicy,
    pub privacy: PrivacyPolicy,
    pub chat_defaults: ChatDefaults,
    pub web_search: WebSearchPolicy,
    pub subagents: SubagentSettings,
    pub shell: ShellPolicy,
    pub external_agents: ExternalAgents,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAgents {
    pub antigravity: ExternalAgent,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAgent {
    pub executable: String,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            network_proxy: NetworkProxy {
                enabled: false,
                protocol: ProxyProtocol::Http,
                host: "127.0.0.1".into(),
                port: 7890,
                auth_enabled: false,
                username: String::new(),
                bypass_list: ["metaso.cn", "baidu.com"].map(String::from).to_vec(),
                auto_bypass_domains: [
                    "localhost",
                    "127.0.0.1",
                    "::1",
                    "192.168.*",
                    "10.*",
                    "*.local",
                ]
                .map(String::from)
                .to_vec(),
            },
            personalization: Personalization {
                display_name: String::new(),
                assistant_tone: String::new(),
            },
            memory: MemoryPolicy {
                enabled: true,
                agent_read_enabled: false,
            },
            workspace_instructions: EnabledPolicy { enabled: true },
            privacy: PrivacyPolicy {
                incognito_active: false,
            },
            chat_defaults: ChatDefaults::default(),
            web_search: WebSearchPolicy {
                enabled: false,
                default_provider: WebSearchProvider::Model,
            },
            subagents: SubagentSettings::default(),
            shell: ShellPolicy {
                preference: ShellPreference::Auto,
                executable: String::new(),
            },
            external_agents: ExternalAgents::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePolicySnapshot {
    pub revision: u64,
    pub policy: RuntimePolicy,
}
impl RuntimePolicySnapshot {
    pub fn validate(&self) -> Result<(), String> {
        decode_canonical_snapshot(serde_json::to_value(self).map_err(|e| e.to_string())?)
            .map(|_| ())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimePolicyMutation {
    SetNetworkProxy { value: NetworkProxy },
    SetPersonalization { value: Personalization },
    SetMemory { value: MemoryPolicy },
    SetWorkspaceInstructions { value: EnabledPolicy },
    SetPrivacy { value: PrivacyPolicy },
    SetChatDefaults { value: ChatDefaults },
    SetWebSearch { value: WebSearchPolicy },
    SetSubagents { value: SubagentSettings },
    SetShell { value: ShellPolicy },
    SetExternalAgents { value: ExternalAgents },
    PatchAgentSettings { value: AgentSettingsPatch },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimePolicyMutationInput {
    pub expected_revision: u64,
    pub operation: RuntimePolicyMutation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimePolicyMutationResult {
    Committed {
        revision: u64,
    },
    RevisionConflict {
        expected_revision: u64,
        actual_revision: u64,
    },
}
