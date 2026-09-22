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

use super::Interactions;
use maka_event_log::interactions::PermissionGrant;
use maka_runtime::event::Invocation;
use maka_runtime::{
    interaction::{InteractionOutcome, InteractionRecord, InteractionRequest, PermissionRequest},
    tool_call::ToolRejection,
};
use maka_sandbox::{ApprovalKind, grant::Decision};
use tokio_util::sync::CancellationToken;

impl Interactions {
    pub(crate) async fn request_permissions(
        &self,
        invocation: &Invocation,
        tool_use_id: Option<&str>,
        request: PermissionRequest,
        base_revision: u64,
        cancellation: &CancellationToken,
    ) -> Result<PermissionGrant, ToolRejection> {
        let record = {
            let _gate = self.admission.lock().await;
            if self.shutdown.is_cancelled() || cancellation.is_cancelled() {
                return Err(ToolRejection::Cancelled);
            }
            request
                .validate()
                .map_err(|message| ToolRejection::InvalidInput {
                    message: message.into(),
                })?;
            let observation = self
                .active_projection(invocation)
                .await
                .map_err(|error| failed(error.message))?;
            if observation.session.configuration.boundary_revision != base_revision {
                return Err(denied("Session permissions changed before approval"));
            }
            let grants = self
                .log
                .permission_grants(invocation, tool_use_id, base_revision)
                .await
                .map_err(|error| failed(self.store_failure(error).message))?;
            for grant in grants {
                if grant
                    .permissions
                    .contains(&request.permissions)
                    .map_err(failed)?
                {
                    return Ok(grant);
                }
            }
            if !observation
                .session
                .configuration
                .approval_policy
                .allows(ApprovalKind::Permissions)
            {
                return Err(denied(
                    "Additional permissions require approval, but the Session policy forbids prompting",
                ));
            }
            let record = InteractionRecord {
                session_id: invocation.session_id.clone(),
                turn_id: invocation.turn_id.clone(),
                run_id: invocation.run_id.clone(),
                request_id: uuid::Uuid::new_v4().to_string(),
                created_at: self.timestamp().map_err(|error| failed(error.message))?,
                request: InteractionRequest::Permissions {
                    tool_use_id: tool_use_id.map(str::to_owned),
                    base_revision,
                    request,
                },
                outcome: None,
            };
            self.publish(observation, record)
                .await
                .map_err(|error| failed(error.message))?
        };
        // Human deliberation owns no admission lock and has no arbitrary TTL.
        let outcome = self
            .wait_for_outcome(&record.request_id, cancellation)
            .await
            .map_err(|error| failed(error.message))?;
        match outcome {
            InteractionOutcome::PermissionsDecision {
                decision: Decision::Allow { .. },
                ..
            } => {
                let _gate = self.admission.lock().await;
                if self.shutdown.is_cancelled() || cancellation.is_cancelled() {
                    return Err(ToolRejection::Cancelled);
                }
                self.log
                    .permission_grants(invocation, tool_use_id, base_revision)
                    .await
                    .map_err(|error| failed(self.store_failure(error).message))?
                    .into_iter()
                    .find(|grant| grant.request_id == record.request_id)
                    .ok_or_else(|| denied("Approved permissions are no longer current"))
            }
            InteractionOutcome::PermissionsDecision {
                decision: Decision::Deny,
                ..
            } => Err(denied("Additional permissions were denied")),
            InteractionOutcome::Closure { .. } => Err(ToolRejection::Cancelled),
            _ => {
                self.shutdown.cancel();
                Err(failed("Canonical permissions outcome changed kind"))
            }
        }
    }
}

fn failed(error: impl std::fmt::Display) -> ToolRejection {
    ToolRejection::PreparationFailed {
        message: error.to_string(),
    }
}
fn denied(message: &str) -> ToolRejection {
    ToolRejection::PolicyDenied {
        message: message.into(),
    }
}
