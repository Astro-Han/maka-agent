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

use super::{Executions, Operation, Prepared, failed};
use maka_client_capability::broker::{CallError, ServiceCall};
use maka_plugins::{
    authorization::Capability,
    call::Scope,
    client_capability::{NOTIFICATION_SERVICE, NOTIFICATION_VERSION, Notification},
    fiber::Context,
};
use maka_runtime::tools::ToolError;
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

impl Executions {
    pub(crate) async fn plugin_notification(
        &self,
        owner: Context,
        call: Scope,
        input: Notification,
        cancellation: CancellationToken,
    ) -> Result<(), ToolError> {
        input.validate().map_err(failed)?;
        let identity = owner.identity().map_err(failed)?;
        let registration = self
            .plugin_resource_clients(&call, Capability::Notifications)
            .await
            .map_err(failed)?
            .into_iter()
            .filter(|registration| {
                registration.available()
                    && registration
                        .manifest()
                        .services
                        .as_ref()
                        .is_some_and(|services| {
                            services.iter().any(|service| {
                                service.service_id == NOTIFICATION_SERVICE
                                    && service.version == NOTIFICATION_VERSION
                            })
                        })
            })
            .min_by(|left, right| left.provider_id().cmp(right.provider_id()))
            .ok_or_else(|| failed("no authorized notification provider is available"))?;
        let operation = Operation::Notification {
            input: input.clone(),
            provider: (**registration.identity()).clone(),
            registration_id: registration.manifest().registration_id.clone(),
        };
        let pending = self
            .capabilities
            .broker
            .prepare_service(
                registration,
                ServiceCall {
                    service_id: NOTIFICATION_SERVICE.into(),
                    version: NOTIFICATION_VERSION.into(),
                    method: "send".into(),
                    input: json!({"packageId":identity.package_id, "notification":input})
                        .as_object()
                        .unwrap()
                        .clone(),
                },
                Duration::from_secs(20),
                // Cancellation may withdraw an offer, never undo a notification
                // already admitted. The broker still enforces its deadline and
                // Host drain; the resource worker owns settlement after start.
                CancellationToken::new(),
            )
            .map_err(failed)?;
        let accepted = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(failed("notification withdrawn before admission")),
            accepted = pending.accepted() => accepted.map_err(failed)?,
        };
        self.journal(
            owner,
            call,
            Prepared {
                operation,
                capability: Capability::Notifications,
                effect: Box::pin(async move {
                    accepted
                        .start()
                        .await
                        .map(|_| serde_json::Value::Null)
                        .map_err(|error| match error {
                            CallError::OutcomeUnknown(_) => {
                                ToolError::CleanupUnconfirmed(error.to_string())
                            }
                            // Provider acceptance is not an assurance that a notification
                            // was unseen. Do not automatically send it a second time.
                            error => ToolError::OutcomeUnknown(error.to_string()),
                        })
                }),
            },
            cancellation,
        )
        .await?;
        Ok(())
    }
}
