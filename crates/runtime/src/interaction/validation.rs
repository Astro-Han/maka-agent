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
use crate::capability::{FormInput, json::encoded_limit};

pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

pub fn entity_id(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err("Invalid interaction entity identity");
    }
    Ok(())
}

impl GrantTarget {
    pub fn validate(&self) -> Result<(), &'static str> {
        for id in [
            &self.provider_id,
            &self.contract_id,
            &self.server_id,
            &self.tool_name,
        ] {
            entity_id(id)?;
        }
        match (&self.capability, &self.scope) {
            (GrantCapability::Browser, GrantScope::BrowserOrigin { origin }) => {
                let url = url::Url::parse(origin).map_err(|_| "Invalid browser origin")?;
                if origin.encode_utf16().count() > 16_384
                    || !matches!(url.scheme(), "http" | "https")
                    || url.origin().ascii_serialization() != *origin
                {
                    return Err("Browser scope must be a canonical HTTP origin");
                }
            }
            (GrantCapability::ComputerUse, GrantScope::Capability {}) => {}
            (
                GrantCapability::DesktopMcp,
                GrantScope::McpTool {
                    server_id,
                    tool_name,
                },
            ) => {
                entity_id(server_id)?;
                entity_id(tool_name)?;
            }
            _ => return Err("Grant scope does not match capability"),
        }
        Ok(())
    }
}

impl InteractionRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Question {
                tool_use_id,
                questions,
            } => {
                if tool_use_id.is_empty() || tool_use_id.len() > 256 {
                    return Err("Invalid interaction tool use identity");
                }
                question::validate_questions(questions)?;
            }
            Self::Form {
                tool_use_id,
                message,
                requester,
                fields,
            } => {
                if tool_use_id.is_empty() || tool_use_id.len() > 256 {
                    return Err("Invalid interaction tool use identity");
                }
                FormInput {
                    message: message.clone(),
                    requester: requester.clone(),
                    fields: fields.clone(),
                }
                .validate()?;
            }
            Self::ClientCapability {
                tool_use_id,
                target,
            } => {
                if tool_use_id.is_empty() || tool_use_id.len() > 256 {
                    return Err("Invalid interaction tool use identity");
                }
                target.validate()?;
            }
        }
        serialized_limit(self, 16 * 1024)
    }
}

impl InteractionOutcome {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.committed_at() > MAX_SAFE_INTEGER {
            return Err("Invalid outcome timestamp");
        }
        if let Self::FormAnswer { result, .. } = self {
            result.validate_values()?;
        }
        if let Self::QuestionAnswer { answers, .. } = self {
            question::validate_answers(answers)?;
        }
        serialized_limit(self, 8 * 1024)
    }

    pub fn validate_for_request(&self, request: &InteractionRequest) -> Result<(), &'static str> {
        request.validate()?;
        self.validate()?;
        match (request, self) {
            (
                InteractionRequest::Form { .. } | InteractionRequest::Question { .. },
                Self::Closure {
                    reason: ClosureReason::TimedOut,
                    ..
                },
            ) => Err("Forms and questions have no human-response deadline"),
            (
                InteractionRequest::Question { questions, .. },
                Self::QuestionAnswer { answers, .. },
            ) => question::validate_answer_count(answers, questions),
            (_, Self::Closure { .. })
            | (
                InteractionRequest::ClientCapability { .. },
                Self::ClientCapabilityDecision { .. },
            ) => Ok(()),
            (InteractionRequest::Form { fields, .. }, Self::FormAnswer { result, .. }) => {
                result.validate_for_fields(fields)
            }
            _ => Err("Interaction outcome does not match request"),
        }
    }
}

impl InteractionAnswer {
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Self::Form { result } = self {
            result.validate()?;
        }
        if let Self::Question { answers } = self {
            question::validate_answers(answers)?;
        }
        serialized_limit(self, 8 * 1024)
    }
    pub fn validate_for_request(&self, request: &InteractionRequest) -> Result<(), &'static str> {
        request.validate()?;
        self.validate()?;
        match (self, request) {
            (Self::Question { answers }, InteractionRequest::Question { questions, .. }) => {
                question::validate_answer_count(answers, questions)
            }
            (Self::ClientCapability { .. }, InteractionRequest::ClientCapability { .. }) => Ok(()),
            (Self::Form { result }, InteractionRequest::Form { fields, .. }) => {
                result.validate_for_fields(fields)
            }
            _ => Err("Interaction answer does not match request"),
        }
    }
}

impl InteractionRecord {
    pub fn validate(&self) -> Result<(), &'static str> {
        for id in [
            &self.session_id,
            &self.turn_id,
            &self.run_id,
            &self.request_id,
        ] {
            entity_id(id)?;
        }
        if self.created_at > MAX_SAFE_INTEGER {
            return Err("Invalid interaction creation timestamp");
        }
        self.request.validate()?;
        if let Some(outcome) = &self.outcome {
            outcome.validate_for_request(&self.request)?;
        }
        Ok(())
    }
}

impl SessionGrant {
    pub fn validate(&self) -> Result<(), &'static str> {
        entity_id(&self.session_id)?;
        self.target.validate()?;
        if self.granted_at > MAX_SAFE_INTEGER {
            return Err("Invalid grant timestamp");
        }
        Ok(())
    }
}

fn serialized_limit(value: &impl Serialize, limit: usize) -> Result<(), &'static str> {
    encoded_limit(value, limit)
}
