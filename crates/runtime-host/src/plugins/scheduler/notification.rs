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

use super::Backend;
use maka_client_capability::broker::ServiceCall;
use maka_plugins::execution::SessionBoundary;
use maka_scheduler::{
    delivery::Delivery,
    plan::Fire,
    task::{Effect, Notification},
};
use serde_json::json;
use std::time::Duration;

const SERVICE: &str = "maka_scheduled_task_native_effect";
impl Backend {
    pub(super) async fn notify(&self, fire: &Fire, source: &Option<SessionBoundary>) -> Delivery {
        let Effect::Notify(notification) = &fire.effect else {
            return Delivery::Failed("invalid notification effect".into());
        };
        let registration = {
            let registry = self
                .capabilities
                .registry
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // Match the workspace-service contract: stable selection among
            // connected owner providers, pinning exactly one publication.
            registry
                .published()
                .filter(|registration| {
                    registration.available()
                        && registration
                            .manifest()
                            .services
                            .as_ref()
                            .is_some_and(|services| {
                                services.iter().any(|service| {
                                    service.service_id == SERVICE && service.version == "1"
                                })
                            })
                })
                .min_by(|left, right| left.provider_id().cmp(right.provider_id()))
                .cloned()
        };
        let Some(registration) = registration else {
            return Delivery::Deferred("Waiting for a notification provider".into());
        };
        let (method, input) = match notification {
            Notification::Local => (
                "notify_local",
                json!({"taskId":fire.task_id,"title":fire.title}),
            ),
            Notification::Bot { platform, chat_id } => (
                "notify_bot",
                json!({"taskId":fire.task_id,"title":fire.title,
                "body":fire.intent.body(),"platform":platform,"chatId":chat_id}),
            ),
        };
        let stop = match self.context.stopping() {
            Ok(stop) => stop,
            Err(error) => return Delivery::Deferred(error.to_string()),
        };
        let pending = match self.capabilities.broker.prepare_service(
            registration,
            ServiceCall {
                service_id: SERVICE.into(),
                version: "1".into(),
                method: method.into(),
                input: input.as_object().unwrap().clone(),
            },
            Duration::from_secs(20),
            stop,
        ) {
            Ok(pending) => pending,
            Err(error) => return Delivery::Deferred(error.to_string()),
        };
        let accepted = match pending.accepted().await {
            Ok(accepted) => accepted,
            Err(error) => return Delivery::Deferred(error.to_string()),
        };
        let host = match self.host() {
            Ok(host) => host,
            Err(error) => return Delivery::Deferred(error.to_string()),
        };
        let gate = host.lock_admission().await;
        let _lease = match self.context.admit() {
            Ok(lease) => lease,
            Err(error) => return Delivery::Deferred(error.to_string()),
        };
        if let Err(error) = self.check_source(source).await {
            return Delivery::Blocked(error.to_string());
        }
        match self.privacy_allows().await {
            Ok(true) => {}
            Ok(false) => {
                return Delivery::Blocked("Scheduled tasks are disabled in incognito mode".into());
            }
            Err(error) => return Delivery::Deferred(error.to_string()),
        }
        let identity = match self.context.identity() {
            Ok(identity) => identity,
            Err(error) => return Delivery::Deferred(error.to_string()),
        };
        let catalog = host
            .plugin_catalog
            .snapshot::<super::Service>(&maka_plugins::composition::Scope::Profile);
        let active = catalog
            .entries
            .get(&identity.entry_id)
            .filter(|entry| {
                entry
                    .owner
                    .identity()
                    .is_ok_and(|owner| owner.activation == identity.activation)
            })
            .and_then(|entry| {
                entry
                    .value
                    .handle
                    .snapshot()
                    .tasks
                    .get(&fire.task_id)
                    .cloned()
            })
            .is_some_and(|task| task.status == maka_scheduler::task::Status::Active);
        if !active {
            return Delivery::Deferred("Notification was paused before admission".into());
        }
        let result = accepted.start();
        drop(gate);
        match result.await {
            Ok(_) => Delivery::Notified,
            Err(error) => {
                Delivery::Failed(format!("Native notification outcome is unknown: {error}"))
            }
        }
    }
}
