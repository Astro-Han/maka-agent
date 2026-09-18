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

use crate::{
    Error, invalid,
    task::{Creator, Effect},
};
use maka_plugins::execution::{RootApproval, SessionBoundary};
use maka_runtime::event::Invocation;
use serde::{Deserialize, Serialize};

/// Supplied by the Host entrypoint, never decoded from a client payload.
#[derive(Clone)]
pub enum Origin {
    User,
    Agent(Invocation),
}
impl Origin {
    pub fn creator(&self) -> Creator {
        match self {
            Self::User => Creator::User,
            Self::Agent(invocation) => Creator::Agent {
                session_id: invocation.session_id.clone(),
            },
        }
    }
}

/// Internal approval data is stored with the plan, outside its public wire Task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Authorization {
    Notification { source: Option<SessionBoundary> },
    Session { boundary: SessionBoundary },
    Root { approval: Box<RootApproval> },
}
impl Authorization {
    pub fn validate(&self, effect: &Effect) -> Result<(), Error> {
        match (self, effect) {
            (Self::Notification { source }, Effect::Notify(_)) => {
                if let Some(source) = source {
                    source
                        .validate()
                        .map_err(|error| invalid(error.to_string()))?;
                }
                Ok(())
            }
            (Self::Session { boundary }, Effect::SessionResume { session_id })
                if &boundary.session_id == session_id =>
            {
                boundary
                    .validate()
                    .map_err(|error| invalid(error.to_string()))
            }
            (Self::Root { approval }, Effect::AgentRun { execution }) => {
                let template = &approval.template;
                template
                    .validate()
                    .map_err(|error| invalid(error.to_string()))?;
                if let Some(source) = &approval.source {
                    source
                        .validate()
                        .map_err(|error| invalid(error.to_string()))?;
                }
                if template.model.connection_id != execution.llm_connection_id
                    || template.model.connection_slug != execution.llm_connection_slug
                    || template.model.model != execution.model
                    || template.permission_mode != execution.permission_mode
                    || template.thinking_level != execution.thinking_level
                    || template.collaboration_mode != execution.collaboration_mode
                    || template.orchestration_mode != execution.orchestration_mode
                {
                    return Err(invalid("execution approval does not match task effect"));
                }
                Ok(())
            }
            _ => Err(invalid("authorization does not match task effect")),
        }
    }
}
