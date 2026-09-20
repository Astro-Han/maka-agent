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

mod authority;
mod children;
mod client;
mod effects;
mod filesystem;
mod llm;
mod root;
use root::RootGrant;
mod scopes;
pub(crate) use scopes::ResourceTarget;
mod workspace;

use super::Executions;
use crate::session::SessionConfiguration;
use futures_util::future::BoxFuture;
use maka_event_log::StoreError;
use maka_plugins::{
    composition::Scope,
    execution::{
        ChildSession, CommandError as Error, Commands, CreateChild, EventPage, Observation,
        Receipt, Submit,
    },
    fiber::Context,
    storage::Namespace,
};
use maka_runtime::{
    event::{EventWrite, Fact, InvocationInput, InvocationOutcome, RuntimeEvent},
    execution::PermissionMode,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

#[derive(Clone)]
struct Grant {
    boundary_revision: u64,
    permission_mode: PermissionMode,
    cwd: String,
}

/// Only Host can construct this capability; the caller supplies an explicit
/// Session allowlist. Empty grants confer no execution authority.
struct BoundCommands {
    executions: Weak<Executions>,
    context: Context,
    namespace: Namespace,
    grants: Arc<Mutex<BTreeMap<String, Grant>>>,
    root_id: String,
    submission_stop: tokio_util::sync::CancellationToken,
    root_grant: Option<RootGrant>,
    consent: Option<maka_plugins::authorization::Id>,
    call: Option<maka_plugins::call::Scope>,
}

impl Executions {
    pub(crate) async fn admit_plugin_process(
        &self,
        scope: &maka_plugins::call::Scope,
    ) -> Result<(String, tokio::sync::OwnedMutexGuard<()>), Error> {
        let gate = self.interactions.own_admission().await;
        let cwd = self
            .plugin_resource_workspace(scope, maka_plugins::authorization::Capability::Processes)
            .await?;
        Ok((cwd, gate))
    }

    pub(crate) async fn plugin_network_policy(
        &self,
        scope: &maka_plugins::call::Scope,
    ) -> Result<maka_network::Policy, Error> {
        // Raw HTTP can have arbitrary external side effects, just like a process.
        // Both the admitted and current permission must still allow them.
        self.plugin_resource_workspace(scope, maka_plugins::authorization::Capability::Network)
            .await?;
        let network = self
            .configuration
            .network_configuration()
            .await
            .map_err(|error| Error::Host(error.to_string()))?;
        maka_network::Policy::from_settings(&network.proxy, network.password.as_deref())
            .map_err(|error| Error::Host(error.to_string()))
    }

    pub(crate) async fn plugin_process_workspace(
        &self,
        invocation: &maka_runtime::event::Invocation,
    ) -> Result<String, Error> {
        if !self.accepting() {
            return Err(Error::Draining);
        }
        let frozen = self
            .log
            .invocation_configuration(invocation)
            .await
            .map_err(storage)?
            .ok_or(Error::Denied)?;
        let current = self
            .log
            .get_session::<SessionConfiguration>(&invocation.session_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        if frozen.permission_mode != PermissionMode::Bypass
            || current.archived
            || current.configuration.permission_mode != PermissionMode::Bypass
            || current.configuration.workspace.host_cwd != frozen.cwd
        {
            return Err(Error::Denied);
        }
        let cwd = frozen.cwd;
        tokio::task::spawn_blocking(move || {
            let identity = maka_fs_tools::workspace::read_identity(std::path::Path::new(&cwd))
                .map_err(|_| Error::Denied)?;
            if frozen.workspace_identity.as_ref() != Some(&identity) {
                return Err(Error::Denied);
            }
            Ok(cwd)
        })
        .await
        .map_err(|error| Error::Host(error.to_string()))?
    }

    pub(crate) fn executor_binding(
        &self,
        session_id: &str,
        id: &maka_runtime::executor::ExecutorId,
    ) -> super::Result<maka_plugins::executor::Binding> {
        let contribution = self
            .plugin_catalog
            .snapshot::<maka_plugins::executor::Executor>(&Scope::Session(session_id.into()))
            .entries
            .remove(id.as_str())
            .ok_or_else(|| {
                super::failure(super::Code::OperationUnavailable, "Executor is not active")
            })?;
        maka_plugins::executor::Binding::new(session_id.into(), contribution)
            .map(|binding| binding.with_calls(self.plugin_calls.clone()))
            .map_err(super::internal)
    }

    pub(crate) fn plugin_store(
        self: &Arc<Self>,
        context: Context,
    ) -> Result<Arc<crate::plugins::storage::BoundStore>, maka_plugins::Error> {
        Ok(Arc::new(crate::plugins::storage::BoundStore::new(
            self.log.clone(),
            self.configuration.clone(),
            context,
            self.workers.clone(),
            self.shutdown.clone(),
        )?))
    }

    async fn observe_plugin(&self, receipt: Receipt) -> Result<Observation, Error> {
        self.log
            .plugin_execution_progress(receipt)
            .await
            .map_err(storage)
    }

    async fn cancel_plugin(self: &Arc<Self>, receipt: Receipt) -> Result<Observation, Error> {
        let admission = self.lock_admission().await;
        if !self.accepting() {
            return Err(Error::Draining);
        }
        let invocation = &receipt.invocation;
        if let Some(pending) = self
            .log
            .message_admission(&invocation.session_id, &receipt.message_id)
            .await
            .map_err(storage)?
        {
            if pending.invocation != *invocation {
                return Err(Error::Conflict);
            }
            // Initial root work is not a mutable queue entry. Settle its accepted
            // identity canonically before releasing admission; no model is started.
            let facts = [
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        content: pending.source.message.content.clone(),
                        request_fingerprint: None,
                        source_messages: vec![pending.source],
                    },
                },
                Fact::InvocationEnded {
                    outcome: InvocationOutcome::Cancelled {
                        source: "plugin_cancellation".into(),
                    },
                },
            ];
            let writes = facts
                .into_iter()
                .map(|fact| EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| Error::Host(e.to_string()))?;
            self.log.append_batch(&writes).await.map_err(|e| {
                self.begin_drain();
                Error::OutcomeUnknown(e.to_string())
            })?;
        } else {
            drop(admission);
            self.stop(maka_protocol::turn::TurnStopInput {
                session_id: invocation.session_id.clone(),
                turn_id: invocation.turn_id.clone(),
                run_id: invocation.run_id.clone(),
            })
            .await
            .map_err(|e| Error::Host(e.message))?;
        }
        self.observe_plugin(receipt).await
    }
}

impl BoundCommands {
    fn executions(&self) -> Result<Arc<Executions>, Error> {
        self.executions
            .upgrade()
            .filter(|host| host.accepting())
            .ok_or(Error::Draining)
    }

    async fn authorize(&self, host: &Executions, session: &str) -> Result<(), Error> {
        self.authorize_origin(host).await?;
        if host
            .log
            .session_manager(session)
            .await
            .map_err(storage)?
            .is_some_and(|manager| manager != self.namespace)
        {
            return Err(Error::Denied);
        }
        let grant = self
            .grants
            .lock()
            .unwrap()
            .get(session)
            .cloned()
            .ok_or(Error::Denied)?;
        let current = host
            .log
            .get_session::<SessionConfiguration>(session)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        if current.archived
            || current.configuration.boundary_revision != grant.boundary_revision
            || current.configuration.permission_mode != grant.permission_mode
            || current.configuration.workspace.host_cwd != grant.cwd
        {
            return Err(Error::Denied);
        }
        Ok(())
    }

    async fn receipt(&self, host: &Executions, operation: &str) -> Result<Receipt, Error> {
        let receipt = host
            .log
            .plugin_execution_receipt(&self.namespace, operation)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        self.authorize(host, &receipt.invocation.session_id).await?;
        Ok(receipt)
    }
}

impl Commands for BoundCommands {
    fn session(
        &self,
        session_id: String,
    ) -> BoxFuture<'_, Result<maka_plugins::session::View, Error>> {
        Box::pin(async move {
            let host = self.executions()?;
            let _lease = self.context.admit().map_err(|_| Error::Revoked)?;
            let _gate = host.lock_admission().await;
            self.authorize(&host, &session_id).await?;
            let record = host
                .log
                .get_session::<SessionConfiguration>(&session_id)
                .await
                .map_err(storage)?
                .ok_or(Error::NotFound)?;
            let configuration = record.configuration;
            let target = match configuration.target {
                crate::session::SessionTarget::Model { model } => {
                    maka_plugins::execution::Target::Model {
                        model,
                        thinking_level: configuration.thinking_level,
                    }
                }
                crate::session::SessionTarget::Executor { executor_id } => {
                    maka_plugins::execution::Target::Executor { executor_id }
                }
            };
            Ok(maka_plugins::session::View {
                session_id: record.id,
                revision: record.revision,
                name: configuration.name,
                boundary_revision: configuration.boundary_revision,
                workspace: configuration.workspace,
                target,
                permission_mode: configuration.permission_mode,
                collaboration_mode: configuration.collaboration_mode,
                behavior: configuration.orchestration_mode,
                tool_mode: configuration.tool_mode,
                bound_tools: configuration.bound_tools,
            })
        })
    }
    fn validate_authority(&self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let host = self.executions()?;
            let _lease = self.context.admit().map_err(|_| Error::Revoked)?;
            self.authorize_origin(&host).await?;
            let sessions = self
                .grants
                .lock()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            for session in sessions {
                self.authorize(&host, &session).await?;
            }
            Ok(())
        })
    }
    fn create_root(
        &self,
        request: maka_plugins::execution::CreateRoot,
    ) -> BoxFuture<'_, Result<ChildSession, Error>> {
        Box::pin(self.root(request))
    }
    fn boundaries(&self) -> Result<Vec<maka_plugins::execution::SessionBoundary>, Error> {
        let _lease = self.context.resource_call().map_err(|_| Error::Revoked)?;
        Ok(self
            .grants
            .lock()
            .unwrap()
            .iter()
            .map(|(id, grant)| maka_plugins::execution::SessionBoundary {
                session_id: id.clone(),
                boundary_revision: grant.boundary_revision,
                permission_mode: grant.permission_mode,
                cwd: grant.cwd.clone(),
            })
            .collect())
    }
    fn workspace_patch(
        &self,
        operation_id: String,
    ) -> BoxFuture<'_, Result<Option<maka_plugins::execution::WorkspacePatch>, Error>> {
        Box::pin(self.export_workspace(operation_id))
    }
    fn event(
        &self,
        operation_id: String,
        event_id: String,
        through: u64,
    ) -> BoxFuture<'_, Result<Option<maka_runtime::event::StoredEvent>, Error>> {
        Box::pin(async move {
            let host = self.executions()?;
            let _lease = self.context.admit().map_err(|_| Error::Revoked)?;
            let receipt = self.receipt(&host, &operation_id).await?;
            host.log
                .execution_event(&receipt.invocation, &event_id, through)
                .await
                .map_err(storage)
        })
    }

    fn changes(&self) -> Result<tokio::sync::watch::Receiver<u64>, Error> {
        let host = self.executions()?;
        let _lease = self.context.resource_call().map_err(|_| Error::Revoked)?;
        Ok(host.log.subscribe_commits())
    }

    fn events(
        &self,
        operation_id: String,
        after: u64,
        through: u64,
    ) -> BoxFuture<'_, Result<EventPage, Error>> {
        Box::pin(async move {
            let host = self.executions()?;
            let _lease = self.context.admit().map_err(|_| Error::Revoked)?;
            let receipt = self.receipt(&host, &operation_id).await?;
            let page = host
                .log
                .session_events(
                    &receipt.invocation.session_id,
                    after,
                    through,
                    128,
                    1024 * 1024,
                )
                .await
                .map_err(storage)?;
            Ok(EventPage {
                // A logical Turn includes handoff successors, not unrelated
                // later work in the same child Session. Cursor still advances
                // over skipped records and never reinterprets a physical Run.
                events: page
                    .events
                    .into_iter()
                    .filter(|row| row.event.invocation.turn_id == receipt.invocation.turn_id)
                    .collect(),
                through_sequence: page.through_sequence,
                next_after: page.next_after,
            })
        })
    }

    fn create_child(&self, request: CreateChild) -> BoxFuture<'_, Result<ChildSession, Error>> {
        Box::pin(self.child(request))
    }

    fn submit(&self, request: Submit) -> BoxFuture<'_, Result<Receipt, Error>> {
        Box::pin(async move {
            request
                .validate()
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let host = self.executions()?;
            let gate = host.interactions.own_admission().await;
            self.authorize(&host, &request.session_id).await?;
            if !host.accepting() {
                return Err(Error::Draining);
            }
            // Even receipt lookups require a currently admitted instance.
            let lease = self.context.admit().map_err(|_| Error::Revoked)?;
            if let Some(receipt) = host
                .log
                .plugin_execution_receipt(&self.namespace, &request.operation_id)
                .await
                .map_err(storage)?
            {
                if receipt.content_digest
                    != request
                        .digest()
                        .map_err(|e| Error::Invalid(e.to_string()))?
                {
                    return Err(Error::Conflict);
                }
                return Ok(receipt);
            }
            if self.submission_stop.is_cancelled() {
                return Err(Error::Revoked);
            }
            if host
                .has_session_work(&request.session_id)
                .await
                .map_err(storage)?
            {
                return Err(Error::Busy);
            }
            host.validate_message_content(
                &request.session_id,
                &request.content.clone().into(),
                &self.root_id,
            )
            .await
            .map_err(|e| Error::Invalid(e.message))?;
            let namespace = self.namespace.clone();
            let (send, receive) = tokio::sync::oneshot::channel();
            // Ownership transfers before the first accepted SQL write. Losing the
            // plugin future cannot release its lease ahead of commit settlement.
            let worker = host.clone();
            host.workers.spawn(async move {
                let result = async {
                    if !worker.accepting() {
                        return Err(Error::Draining);
                    }
                    worker
                        .log
                        .admit_plugin_execution(&namespace, request)
                        .await
                        .map_err(|e| {
                            if matches!(
                                e,
                                StoreError::CommitUnknown(_) | StoreError::OperationUnknown
                            ) {
                                worker.begin_drain();
                            }
                            storage(e)
                        })
                }
                .await;
                drop(gate);
                drop(lease);
                let receipt = result.as_ref().ok().cloned();
                let _ = send.send(result);
                let mut gate = Some(worker.lock_admission().await);
                if let Some(receipt) = receipt
                    && let Err(error) = worker
                        .dispatch_pending(&receipt.invocation.session_id, &mut gate)
                        .await
                {
                    eprintln!("plugin execution dispatch failed: {}", error.message);
                }
            });
            receive
                .await
                .map_err(|_| Error::OutcomeUnknown("Host command owner disappeared".into()))?
        })
    }

    fn query(&self, operation_id: String) -> BoxFuture<'_, Result<Observation, Error>> {
        Box::pin(async move {
            let host = self.executions()?;
            let _lease = self.context.admit().map_err(|_| Error::Revoked)?;
            let receipt = self.receipt(&host, &operation_id).await?;
            host.observe_plugin(receipt).await
        })
    }

    fn cancel(&self, operation_id: String) -> BoxFuture<'_, Result<Observation, Error>> {
        Box::pin(async move {
            let host = self.executions()?;
            let lease = self.context.admit().map_err(|_| Error::Revoked)?;
            let receipt = self.receipt(&host, &operation_id).await?;
            let (send, receive) = tokio::sync::oneshot::channel();
            let worker = host.clone();
            host.workers.spawn(async move {
                let result = worker.cancel_plugin(receipt).await;
                drop(lease);
                let _ = send.send(result);
            });
            receive
                .await
                .map_err(|_| Error::OutcomeUnknown("Host cancellation owner disappeared".into()))?
        })
    }
}

fn storage(error: StoreError) -> Error {
    match error {
        StoreError::EventConflict | StoreError::SessionConflict => Error::Conflict,
        StoreError::SessionBusy => Error::Busy,
        StoreError::SessionNotFound => Error::NotFound,
        StoreError::CommitUnknown(_) | StoreError::OperationUnknown => {
            Error::OutcomeUnknown(error.to_string())
        }
        _ => Error::Host(error.to_string()),
    }
}
