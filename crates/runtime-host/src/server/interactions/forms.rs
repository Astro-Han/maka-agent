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
use maka_client_capability::broker::CallError;
use maka_runtime::{
    capability::{FormInput, FormResult},
    interaction::{InteractionOutcome, InteractionRequest},
};
use maka_tools::ToolCallContext;
use tokio_util::sync::CancellationToken;

impl Interactions {
    pub(super) async fn request_form(
        &self,
        context: ToolCallContext,
        input: FormInput,
        cancellation: CancellationToken,
    ) -> Result<FormResult, CallError> {
        let request = InteractionRequest::Form {
            tool_use_id: context.tool_use_id(),
            message: input.message,
            requester: input.requester,
            fields: input.fields,
        };
        let record = self
            .admit_request(context, request, &cancellation)
            .await
            .map_err(|_| CallError::OutcomeUnknown("form request could not be published"))?;
        match self
            .wait_for_outcome(&record.request_id, &cancellation)
            .await
            .map_err(|_| CallError::OutcomeUnknown("form authority could not be resolved"))?
        {
            InteractionOutcome::FormAnswer { result, .. } => Ok(result),
            InteractionOutcome::Closure { .. } => Err(CallError::Cancelled),
            InteractionOutcome::ClientCapabilityDecision { .. }
            | InteractionOutcome::PermissionsDecision { .. }
            | InteractionOutcome::QuestionAnswer { .. } => {
                self.shutdown.cancel();
                Err(CallError::OutcomeUnknown(
                    "canonical form outcome changed kind",
                ))
            }
        }
    }
}
