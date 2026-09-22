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

use crate::ToolCallContext;
use maka_client_capability::broker::{FormFuture, FormHandler};
use maka_runtime::{capability::FormInput, interaction::GrantTarget, tool_call::ToolRejection};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

pub type ApprovalFuture = Pin<Box<dyn Future<Output = Result<(), ToolRejection>> + Send>>;
pub type PermissionFuture = Pin<
    Box<dyn Future<Output = Result<maka_runtime::execution::SandboxMode, ToolRejection>> + Send>,
>;

/// Host owns canonical decisions; tools supply the exact immutable call identity.
pub trait ClientInteractions: Send + Sync {
    /// Capture the durable permission selection for this call, not its Run opening.
    fn sandbox_mode(&self, context: ToolCallContext) -> PermissionFuture;

    fn approve(
        &self,
        target: GrantTarget,
        context: ToolCallContext,
        cancellation: CancellationToken,
        provider: CancellationToken,
    ) -> ApprovalFuture;

    /// Completion after cancellation includes withdrawal of any pending request.
    fn form(
        &self,
        context: ToolCallContext,
        input: FormInput,
        cancellation: CancellationToken,
    ) -> FormFuture;
}

pub(super) struct BoundForms {
    pub interactions: Arc<dyn ClientInteractions>,
    pub context: ToolCallContext,
}
impl FormHandler for BoundForms {
    fn request(&self, input: FormInput, cancellation: CancellationToken) -> FormFuture {
        self.interactions
            .form(self.context.clone(), input, cancellation)
    }
}
