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

use super::{CallResult, FormInput, FormResult};
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdmissionEvidence {
    None,
    BrowserUrl { url: String },
}

/// Provider-to-host frames. Each variant carries only its admitted wire fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum ClientFrame {
    #[serde(rename = "client.capability.accepted")]
    Accepted {
        invocation_id: String,
        admission_evidence: AdmissionEvidence,
    },
    #[serde(rename = "client.capability.rejected")]
    Rejected {
        invocation_id: String,
        message: String,
    },
    #[serde(rename = "client.capability.failed")]
    Failed {
        invocation_id: String,
        message: String,
    },
    #[serde(rename = "client.capability.progress")]
    Progress {
        invocation_id: String,
        current: u64,
        total: u64,
    },
    #[serde(rename = "client.capability.result")]
    Result {
        invocation_id: String,
        result: CallResult,
    },
    #[serde(rename = "client.capability.result_start")]
    ResultStart {
        invocation_id: String,
        byte_length: u64,
        chunk_count: u64,
    },
    #[serde(rename = "client.capability.result_chunk")]
    ResultChunk {
        invocation_id: String,
        index: u64,
        data: String,
    },
    #[serde(rename = "client.capability.interaction_request")]
    InteractionRequest {
        invocation_id: String,
        interaction_id: String,
        request: FormInput,
    },
}

impl ClientFrame {
    pub fn invocation_id(&self) -> &str {
        match self {
            Self::Accepted { invocation_id, .. }
            | Self::Rejected { invocation_id, .. }
            | Self::Failed { invocation_id, .. }
            | Self::Progress { invocation_id, .. }
            | Self::Result { invocation_id, .. }
            | Self::ResultStart { invocation_id, .. }
            | Self::ResultChunk { invocation_id, .. }
            | Self::InteractionRequest { invocation_id, .. } => invocation_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum HostFrame {
    #[serde(rename = "client.capability.call")]
    Call {
        invocation_id: String,
        registration_id: String,
        offer_id: String,
        server_id: String,
        tool_name: String,
        arguments: Map<String, Value>,
        session_id: String,
        turn_id: String,
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    #[serde(rename = "client.capability.service_call")]
    ServiceCall {
        invocation_id: String,
        registration_id: String,
        service_id: String,
        version: String,
        method: String,
        input: Map<String, Value>,
    },
    #[serde(rename = "client.capability.cancel")]
    Cancel { invocation_id: String },
    #[serde(rename = "client.capability.release")]
    Release { invocation_id: String },
    #[serde(rename = "client.capability.registration_release")]
    RegistrationRelease { registration_id: String },
    #[serde(rename = "client.capability.admitted")]
    Admitted { invocation_id: String },
    #[serde(rename = "client.capability.interaction_result")]
    InteractionResult {
        invocation_id: String,
        interaction_id: String,
        result: FormResult,
    },
}
