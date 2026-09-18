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
use futures_util::future::BoxFuture;
use maka_plugins::execution::{CommandError, CreateRoot, Submit};
use maka_scheduler::{
    Error,
    authorization::{Authorization, Origin},
    delivery::{Delivery, Dispatcher},
    plan::Fire,
    task::Effect,
};

impl Dispatcher for Backend {
    fn authorize(
        &self,
        origin: Origin,
        effect: Effect,
    ) -> BoxFuture<'_, Result<Authorization, Error>> {
        Box::pin(Backend::authorize(self, origin, effect))
    }
    fn dispatch(&self, fire: Fire) -> BoxFuture<'_, Delivery> {
        Box::pin(async move {
            match self.privacy_allows().await {
                Ok(true) => {}
                Ok(false) => {
                    return Delivery::Blocked(
                        "Scheduled tasks are disabled in incognito mode".into(),
                    );
                }
                Err(error) => return Delivery::Deferred(error.to_string()),
            }
            let Some(authorization) = &fire.authorization else {
                return Delivery::Blocked(
                    "Task has no persisted execution approval; explicitly update its target".into(),
                );
            };
            if let Err(error) = authorization.validate(&fire.effect) {
                return Delivery::Blocked(error.to_string());
            }
            if let Authorization::Notification { source } = authorization {
                if let Err(error) = self.check_source(source).await {
                    return classify(error);
                }
                return self.notify(&fire, source).await;
            }
            match self.execute(&fire).await {
                Ok(receipt) => Delivery::Accepted {
                    session_id: receipt.invocation.session_id,
                    run_id: receipt.invocation.run_id,
                },
                Err(error) => classify(error),
            }
        })
    }
}
impl Backend {
    async fn execute(&self, fire: &Fire) -> Result<maka_plugins::execution::Receipt, CommandError> {
        let host = self.host()?;
        let stopping = self.context.stopping().map_err(|_| CommandError::Revoked)?;
        let (commands, session_id) =
            match fire.authorization.as_ref().ok_or(CommandError::Denied)? {
                Authorization::Session { boundary } => {
                    let commands = host.restore_plugin_authority(
                        self.context.clone(),
                        vec![boundary.clone()],
                        &self.root_id,
                        stopping,
                    )?;
                    (commands, boundary.session_id.clone())
                }
                Authorization::Root { approval } => {
                    let commands = host.authorize_plugin_root(
                        self.context.clone(),
                        *approval.clone(),
                        &self.root_id,
                        stopping,
                    )?;
                    let session = commands
                        .create_root(CreateRoot {
                            operation_id: fire.id.clone(),
                            name: fire.title.clone(),
                        })
                        .await?;
                    (commands, session.session_id)
                }
                Authorization::Notification { .. } => {
                    return Err(CommandError::Invalid(
                        "notification is not an execution".into(),
                    ));
                }
            };
        commands
            .submit(Submit {
                orchestration_mode: None,
                operation_id: fire.id.clone(),
                session_id,
                content: fire.intent.body().into(),
            })
            .await
    }
}
fn classify(error: CommandError) -> Delivery {
    match error {
        CommandError::Busy
        | CommandError::Draining
        | CommandError::Revoked
        | CommandError::OutcomeUnknown(_)
        | CommandError::Host(_) => Delivery::Retry(error.to_string()),
        CommandError::Denied | CommandError::NotFound => Delivery::Blocked(error.to_string()),
        CommandError::Conflict | CommandError::Invalid(_) => Delivery::Failed(error.to_string()),
    }
}
