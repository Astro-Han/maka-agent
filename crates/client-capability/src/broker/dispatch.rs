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

use super::{Broker, CallError, PendingCall};
use crate::Registration;
use maka_runtime::capability::{CallSource, HostFrame, HostPathAccess};
use serde_json::{Map, Value};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

pub struct ServiceCall {
    pub service_id: String,
    pub version: String,
    pub method: String,
    pub input: Map<String, Value>,
}

/// Names are resolved against an immutable registration, never against the
/// provider's current publication. Host paths come only from invocation context.
pub struct ToolCall {
    pub offer_id: String,
    pub server_id: String,
    pub tool_name: String,
    pub arguments: Map<String, Value>,
    pub source: CallSource,
    pub tool_call_id: String,
    pub cwd: String,
}

impl Broker {
    pub fn prepare_service(
        &self,
        registration: Arc<Registration>,
        call: ServiceCall,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<PendingCall, CallError> {
        if !registration
            .manifest()
            .services
            .as_ref()
            .is_some_and(|services| {
                services
                    .iter()
                    .any(|s| s.service_id == call.service_id && s.version == call.version)
            })
        {
            return Err(CallError::Invalid(
                "service is not in the pinned registration",
            ));
        }
        let registration_id = registration.manifest().registration_id.clone();
        self.prepare(registration, timeout, cancellation, |invocation_id| {
            HostFrame::ServiceCall {
                invocation_id,
                registration_id,
                service_id: call.service_id,
                version: call.version,
                method: call.method,
                input: call.input,
            }
        })
    }

    /// Obtain provider acceptance before Host policy and T1. Only the returned
    /// accepted handle can cross the execution cut after those checks succeed.
    pub fn prepare_tool(
        &self,
        registration: Arc<Registration>,
        call: ToolCall,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<PendingCall, CallError> {
        if !registration.visible_to(call.source.session_id()) {
            return Err(CallError::Invalid(
                "tool publication belongs to another Session",
            ));
        }
        let offer = registration
            .manifest()
            .offers
            .iter()
            .find(|offer| {
                offer.offer_id == call.offer_id
                    && offer
                        .tools
                        .iter()
                        .any(|tool| tool.server_id == call.server_id && tool.name == call.tool_name)
            })
            .ok_or(CallError::Invalid("tool is not in the pinned registration"))?;
        // Path-independent client offers must never receive the host workspace.
        let cwd = (offer.host_path_access == HostPathAccess::Cwd).then_some(call.cwd);
        let registration_id = registration.manifest().registration_id.clone();
        self.prepare(registration, timeout, cancellation, |invocation_id| {
            HostFrame::Call {
                invocation_id,
                registration_id,
                offer_id: call.offer_id,
                server_id: call.server_id,
                tool_name: call.tool_name,
                arguments: call.arguments,
                source: call.source,
                tool_call_id: call.tool_call_id,
                cwd,
            }
        })
    }
}
