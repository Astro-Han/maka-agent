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
    Error,
    authorization::{Authorization, Origin},
    delivery::{Delivery, Dispatcher},
    task::{Effect, ExecutionTemplate, Notification},
};
use futures_util::future::BoxFuture;
use maka_plugins::{
    authorization::{Capability, Grant, Id, Target},
    call,
    execution::{CommandError, CreateRoot, RootSettings, Submit},
    host::Services,
    storage::{Data, Mutation, StoreError},
};
use maka_runtime::{execution::ModelBinding, tools::ToolError};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const GRANTS: &str = "background-authorizations";

#[derive(Clone)]
pub(super) struct Backend {
    pub host: Services,
}
impl Backend {
    pub async fn grants(&self) -> Result<Vec<Grant>, Error> {
        self.host
            .storage
            .read(GRANTS.into())
            .await?
            .and_then(|record| record.data.value().cloned())
            .map(|value| serde_json::from_value(value).map_err(invalid))
            .unwrap_or_else(|| Ok(vec![]))
    }
    async fn remember(&self, grant: Grant) -> Result<(), Error> {
        for _ in 0..8 {
            let old = self.host.storage.read(GRANTS.into()).await?;
            let mut grants: Vec<Grant> = old
                .as_ref()
                .and_then(|record| record.data.value())
                .map(|data| serde_json::from_value(data.clone()))
                .transpose()
                .map_err(invalid)?
                .unwrap_or_default();
            grants.retain(|saved| {
                saved.id != grant.id
                    && (saved.request.target != grant.request.target
                        || saved.request.capabilities != grant.request.capabilities)
            });
            grants.push(grant.clone());
            if grants.len() > 64 {
                return Err(invalid("background authorization limit reached"));
            }
            match self
                .host
                .storage
                .batch(vec![Mutation {
                    key: GRANTS.into(),
                    expected_revision: old.map(|record| record.revision),
                    data: Data::Present(json!(grants)),
                }])
                .await
            {
                Ok(_) => return Ok(()),
                Err(StoreError::Conflict { .. }) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(Error::Busy)
    }
    pub async fn template(&self, call: call::Scope) -> Result<ExecutionTemplate, Error> {
        let invocation = call
            .identity
            .agent()
            .ok_or_else(|| invalid("expected an Agent call"))?;
        let id = invocation.session_id.clone();
        let commands = self
            .host
            .executions
            .acquire(call)
            .await
            .map_err(Error::from)?;
        let view = commands.session(id).await.map_err(Error::from)?;
        let maka_plugins::execution::Target::Model {
            model,
            thinking_level,
        } = view.target
        else {
            return Err(invalid(
                "scheduled model execution requires a model Session",
            ));
        };
        Ok(ExecutionTemplate {
            project_id: match view.workspace.target {
                maka_runtime::execution::WorkspaceTarget::Project { project_id } => {
                    Some(project_id)
                }
                _ => None,
            },
            cwd: view.workspace.host_cwd,
            llm_connection_id: model.connection_id,
            llm_connection_slug: model.connection_slug,
            model: model.model,
            thinking_level,
            sandbox_mode: view.sandbox_mode,
            approval_policy: view.approval_policy,
            collaboration_mode: view.collaboration_mode,
            orchestration_mode: view.behavior,
            tool_mode: view.tool_mode,
        })
    }
    pub async fn remember_grant(&self, id: Id) -> Result<Grant, Error> {
        let authorized = self
            .host
            .authorizations
            .open(id)
            .await
            .map_err(Error::from)?;
        let grant = authorized.grant;
        authorized.call.finish().await.map_err(Error::from)?;
        if !grant.request.capabilities.iter().any(|capability| {
            matches!(
                capability,
                Capability::Executions | Capability::Notifications
            )
        }) {
            return Err(invalid("grant has no scheduling capability"));
        }
        self.remember(grant.clone()).await?;
        Ok(grant)
    }
    fn matches(grant: &Grant, effect: &Effect) -> bool {
        if grant.revoked {
            return false;
        }
        match effect {
            Effect::Notify(_) => {
                grant
                    .request
                    .capabilities
                    .contains(&Capability::Notifications)
                    && matches!(
                        grant.request.target,
                        Target::Profile | Target::Session { .. } | Target::Workspace { .. }
                    )
            }
            Effect::SessionResume { session_id } => {
                grant.request.capabilities.contains(&Capability::Executions)
                    && matches!(&grant.request.target, Target::Session { session_id: granted } if granted == session_id)
            }
            Effect::AgentRun { execution } => {
                grant.request.capabilities.contains(&Capability::Executions)
                    && matches!(&grant.request.target, Target::Workspace { workspace, sandbox_mode }
                    if *sandbox_mode == execution.sandbox_mode && match workspace {
                        maka_runtime::execution::WorkspaceTarget::Project { project_id } => execution.project_id.as_ref() == Some(project_id),
                        maka_runtime::execution::WorkspaceTarget::HostPath { .. } => execution.project_id.is_none(),
                    })
            }
        }
    }
    async fn approved(&self, id: Id, effect: &Effect) -> Result<Grant, Error> {
        let authorized = self
            .host
            .authorizations
            .open(id)
            .await
            .map_err(Error::from)?;
        let grant = authorized.grant;
        let valid = Self::matches(&grant, effect)
            && match (effect, &authorized.boundary) {
                (
                    Effect::AgentRun { execution },
                    maka_plugins::authorization::Boundary::Workspace { workspace, .. },
                ) => {
                    execution.cwd == workspace.host_cwd
                        || matches!(&grant.request.target,
                    Target::Workspace { workspace:maka_runtime::execution::WorkspaceTarget::HostPath { path }, .. }
                    if path == &execution.cwd)
                }
                (Effect::AgentRun { .. }, _) => false,
                _ => true,
            };
        authorized.call.finish().await.map_err(Error::from)?;
        if !valid {
            return Err(invalid(
                "background authorization does not cover the task target",
            ));
        }
        if let Effect::SessionResume { session_id } = effect {
            let commands = self
                .host
                .executions
                .restore(id)
                .await
                .map_err(Error::from)?;
            if !matches!(
                commands
                    .session(session_id.clone())
                    .await
                    .map_err(Error::from)?
                    .target,
                maka_plugins::execution::Target::Model { .. }
            ) {
                return Err(invalid("scheduled resume requires a model Session"));
            }
        }
        Ok(grant)
    }
    async fn authorization(&self, origin: Origin, effect: Effect) -> Result<Authorization, Error> {
        let explicit = match &origin {
            Origin::User { grant } => *grant,
            Origin::Agent(call) => {
                let invocation = call
                    .identity
                    .agent()
                    .ok_or_else(|| invalid("expected an Agent call"))?;
                let commands = self
                    .host
                    .executions
                    .acquire(call.clone())
                    .await
                    .map_err(Error::from)?;
                let source = commands
                    .session(invocation.session_id.clone())
                    .await
                    .map_err(Error::from)?;
                match &effect {
                    Effect::SessionResume { session_id } if session_id != &source.session_id => {
                        return Err(invalid("an Agent may schedule only its own Session"));
                    }
                    Effect::AgentRun { execution }
                        if execution.cwd != source.workspace.host_cwd
                            || execution.sandbox_mode != source.sandbox_mode
                            || execution.approval_policy != source.approval_policy =>
                    {
                        return Err(invalid(
                            "scheduled target differs from the Agent's authorized workspace",
                        ));
                    }
                    _ => {}
                }
                None
            }
        };
        if let Some(id) = explicit {
            let grant = self.approved(id, &effect).await?;
            self.remember(grant).await?;
            return Ok(Authorization { grant: id });
        }
        let session = origin
            .agent()?
            .map(|invocation| invocation.session_id.as_str());
        for grant in self.grants().await? {
            if !Self::matches(&grant, &effect) {
                continue;
            }
            if let Target::Session { session_id } = &grant.request.target
                && session.is_some_and(|id| id != session_id)
            {
                continue;
            }
            match self.approved(grant.id, &effect).await {
                Ok(_) => return Ok(Authorization { grant: grant.id }),
                Err(
                    Error::Authority(
                        CommandError::Revoked | CommandError::Denied | CommandError::NotFound,
                    )
                    | Error::Invalid(_),
                ) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(invalid(
            "Approve background access in the Scheduler UI before creating this task",
        ))
    }
    async fn execute(
        &self,
        fire: &crate::plan::Fire,
        id: Id,
    ) -> Result<maka_plugins::execution::Receipt, CommandError> {
        let commands = self.host.executions.restore(id).await?;
        let session_id = match &fire.effect {
            Effect::SessionResume { session_id } => session_id.clone(),
            Effect::AgentRun { execution } => {
                commands
                    .create_root(CreateRoot {
                        managed: false,
                        operation_id: fire.id.clone(),
                        name: fire.title.clone(),
                        settings: RootSettings {
                            target: maka_plugins::execution::Target::Model {
                                model: ModelBinding {
                                    connection_id: execution.llm_connection_id.clone(),
                                    connection_slug: execution.llm_connection_slug.clone(),
                                    model: execution.model.clone(),
                                },
                                thinking_level: execution.thinking_level,
                            },
                            sandbox_mode: execution.sandbox_mode,
                            approval_policy: execution.approval_policy,
                            tool_mode: execution.tool_mode,
                            collaboration_mode: execution.collaboration_mode,
                            behavior: execution.orchestration_mode.clone(),
                            bound_tools: None,
                            instructions: None,
                        },
                    })
                    .await?
                    .session_id
            }
            Effect::Notify(_) => {
                return Err(CommandError::Invalid(
                    "notification is not an execution".into(),
                ));
            }
        };
        commands
            .submit(Submit {
                operation_id: fire.id.clone(),
                session_id,
                content: fire.intent.body().into(),
                orchestration_mode: None,
            })
            .await
    }
}
impl Dispatcher for Backend {
    fn authorize(
        &self,
        origin: Origin,
        effect: Effect,
    ) -> BoxFuture<'_, Result<Authorization, Error>> {
        Box::pin(self.authorization(origin, effect))
    }
    fn dispatch(
        &self,
        fire: crate::plan::Fire,
        notification_stop: CancellationToken,
    ) -> BoxFuture<'_, Delivery> {
        Box::pin(async move {
            let Some(authorization) = fire.authorization else {
                return Delivery::Blocked("Task has no background authorization".into());
            };
            if let Effect::Notify(notification) = &fire.effect {
                let authorized = match self.host.authorizations.open(authorization.grant).await {
                    Ok(authorized) => authorized,
                    Err(error) => return classify(error),
                };
                if !Self::matches(&authorized.grant, &fire.effect) {
                    if let Err(error) = authorized.call.finish().await {
                        return Delivery::Failed(error.to_string());
                    }
                    return Delivery::Blocked("Notification authorization changed".into());
                }
                let call = authorized.call.scope();
                let cancelled = call.cancellation.clone();
                let notification = maka_plugins::client_capability::Notification {
                    id: fire.id.clone(),
                    title: fire.title,
                    body: fire.intent.body().into(),
                    destination: match notification {
                        Notification::Local => maka_plugins::client_capability::Destination::Local,
                        Notification::Bot { platform, chat_id } => {
                            maka_plugins::client_capability::Destination::Channel {
                                channel: serde_json::to_value(platform)
                                    .expect("bot enum")
                                    .as_str()
                                    .unwrap()
                                    .into(),
                                recipient: chat_id.clone(),
                            }
                        }
                    },
                };
                let send = self.host.clients.notify(call, notification);
                tokio::pin!(send);
                let result = tokio::select! {
                    biased;
                    _ = notification_stop.cancelled() => { cancelled.cancel(); send.await }
                    result = &mut send => result,
                };
                if let Err(error) = authorized.call.finish().await {
                    return Delivery::Failed(error.to_string());
                }
                return match result {
                    Ok(()) => Delivery::Notified,
                    Err(ToolError::Failed(message)) => Delivery::Deferred(message),
                    Err(error) => Delivery::Failed(error.to_string()),
                };
            }
            match self.execute(&fire, authorization.grant).await {
                Ok(receipt) => Delivery::Accepted {
                    session_id: receipt.invocation.session_id,
                    run_id: receipt.invocation.run_id,
                },
                Err(error) => classify(error),
            }
        })
    }
}
fn classify(error: CommandError) -> Delivery {
    match error {
        CommandError::Busy
        | CommandError::Draining
        | CommandError::OutcomeUnknown(_)
        | CommandError::Unavailable(_)
        | CommandError::Host(_) => Delivery::Retry(error.to_string()),
        CommandError::Revoked | CommandError::Denied | CommandError::NotFound => {
            Delivery::Blocked(error.to_string())
        }
        CommandError::Conflict | CommandError::Invalid(_) => Delivery::Failed(error.to_string()),
    }
}
fn invalid(error: impl ToString) -> Error {
    Error::Invalid(error.to_string())
}
