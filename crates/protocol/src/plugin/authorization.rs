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

use maka_plugins::{
    authorization::{Grant, Id, Request, Target},
    composition::Scope,
    remote::ClientIdentity,
};
use serde::{Deserialize, Serialize};

/// Sent by the application's consent UI, not the plugin Host bridge.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizationInput {
    pub client: ClientIdentity,
    pub scope: Scope,
    pub command: AuthorizationCommand,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthorizationCommand {
    Approve { request: Request },
    Query { id: Id },
    Revoke { id: Id },
}
impl AuthorizationInput {
    pub fn validate(&self) -> crate::Result<()> {
        super::remote::validate_client(&self.client)?;
        if matches!(self.scope, Scope::DesktopUi) {
            return Err(crate::ProtocolError::invalid(
                "authorization requires a Host scope",
            ));
        }
        if let AuthorizationCommand::Approve { request } = &self.command {
            request
                .validate()
                .map_err(|error| crate::ProtocolError::invalid(error.to_string()))?;
            if let Scope::Session(id) = &self.scope
                && !matches!(&request.target, Target::Session { session_id } if session_id == id)
            {
                return Err(crate::ProtocolError::invalid(
                    "Session-scoped authorization cannot escape its Session",
                ));
            }
        }
        Ok(())
    }
    pub fn uses_host_paths(&self) -> bool {
        matches!(
            &self.command,
            AuthorizationCommand::Approve {
                request: Request {
                    target: Target::Workspace {
                        workspace: maka_runtime::execution::WorkspaceTarget::HostPath { .. },
                        ..
                    },
                    ..
                }
            }
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthorizationResult {
    Grant { grant: Option<Grant> },
    Revoked,
}
