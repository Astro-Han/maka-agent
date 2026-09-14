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

use maka_runtime::configuration::ApiProtocol;
use serde::{Deserialize, Serialize};

/// Source-owned execution contracts, including adapters not yet implemented by the host.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAdapter {
    #[serde(flatten)]
    pub kind: AdapterKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_patch_protocol: Option<ApplyPatchProtocol>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum AdapterKind {
    Openai {
        #[serde(skip_serializing_if = "Option::is_none")]
        api_protocol: Option<ApiProtocol>,
    },
    OpenaiCodex,
    OpenaiCompatible {
        name: AdapterName,
        #[serde(skip_serializing_if = "Option::is_none")]
        include_usage: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        require_base_url: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        replay_assistant_reasoning_as: Option<ReasoningField>,
        #[serde(skip_serializing_if = "Option::is_none")]
        replay_assistant_reasoning_details: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        normalize_usage: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        normalize_base_url: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        responses: Option<ResponsesContract>,
        #[serde(skip_serializing_if = "Option::is_none")]
        runtime_profile: Option<RuntimeProfile>,
    },
    Anthropic {
        auth: AnthropicAuth,
        normalize_base_url: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        include_beta_headers: Option<bool>,
    },
    Google {
        #[serde(skip_serializing_if = "Option::is_none")]
        normalize_base_url: Option<bool>,
    },
    Cohere,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterName {
    Provider,
    Connection,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AnthropicAuth {
    ApiKey,
    Bearer,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApplyPatchProtocol {
    OpenaiStructured,
    CodexV4aFreeform,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeProfile {
    AlibabaTokenPlan,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningField {
    Reasoning,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponsesContract {
    pub adapter: ResponsesAdapter,
    pub reasoning_replay: ReasoningReplay,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResponsesAdapter {
    Openai,
    OpenResponses,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningReplay {
    EncryptedContent,
    PlaintextContent,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRuntimeOverride {
    pub adapter: RuntimeAdapter,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}
