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

use serde::{Deserialize, Serialize};

/// Enrollment providers. This is not the model wire protocol:
/// each provider may use a different device grant, token exchange or entitlement check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    OpenaiCodex,
    GithubCopilot,
    XaiOauth,
}
impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCodex => "openai-codex",
            Self::GithubCopilot => "github-copilot",
            Self::XaiOauth => "xai-oauth",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Target {
    Create {
        provider_type: Provider,
        #[serde(skip_serializing_if = "Option::is_none")]
        slug: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Existing {
        connection_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoginStart {
    pub attempt_id: String,
    pub target: Target,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attempt {
    pub attempt_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionIdentity {
    pub connection_id: String,
    pub slug: String,
    pub provider_type: Provider,
}
impl Target {
    pub fn validate_create_identity(&self) -> Result<(), String> {
        use crate::configuration::validation;
        if let Self::Create {
            provider_type,
            slug,
            name,
        } = self
        {
            if *provider_type != Provider::OpenaiCodex && (slug.is_some() || name.is_some()) {
                return Err("Custom OAuth identity is only supported for Codex".into());
            }
            if let Some(slug) = slug {
                validation::slug(slug)?;
            }
            if let Some(name) = name {
                validation::text(name, 256, false)?;
            }
        }
        Ok(())
    }

    pub fn matches(&self, connection: &ConnectionIdentity) -> bool {
        match self {
            Self::Create {
                provider_type,
                slug,
                ..
            } => {
                *provider_type == connection.provider_type
                    && slug.as_ref().is_none_or(|slug| *slug == connection.slug)
            }
            Self::Existing { connection_id } => *connection_id == connection.connection_id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    CapabilityUnavailable,
    AuthorizationFailed,
    ProviderRejected,
    SlugTaken,
    CredentialChanged,
    ConnectionChanged,
    PersistenceFailed,
    InternalFailure,
}

/// Failure cannot accompany a successful or pending login.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    AwaitingAuthorization,
    Exchanging,
    Committing,
    Authenticated,
    Cancelled,
    Failed { failure: Failure },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginProjection {
    pub attempt_id: String,
    pub connection: ConnectionIdentity,
    #[serde(flatten)]
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentQuery {
    pub provider: Provider,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentProjection {
    pub provider: Provider,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "method",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PresentationRequest {
    OpenExternal {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        state_hint: Option<String>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PresentationResult {
    Presented,
}
