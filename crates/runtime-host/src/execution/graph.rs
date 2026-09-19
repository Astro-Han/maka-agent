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
use super::Executions;
use crate::{
    plugins::graph::{
        Root, Submission,
        host::{Activation, Opened, Preferences, Sessions},
    },
    session::SessionConfiguration,
};
use futures_util::future::BoxFuture;
use maka_graph::GraphId;
use maka_plugins::{composition::Scope, fiber::Context, remote::Error};
use std::sync::{Arc, Weak};
use tokio_util::sync::CancellationToken;

pub(crate) struct GraphSessions {
    executions: Weak<Executions>,
    root: String,
}
impl GraphSessions {
    pub(crate) fn new(executions: &Arc<Executions>, root: String) -> Arc<Self> {
        Arc::new(Self {
            executions: Arc::downgrade(executions),
            root,
        })
    }
    fn host(&self) -> Result<Arc<Executions>, String> {
        self.executions
            .upgrade()
            .ok_or_else(|| "Host is closed".into())
    }
}
impl Sessions for GraphSessions {
    fn activate(&self, request: Activation) -> BoxFuture<'_, Result<Root, String>> {
        Box::pin(async move {
            let host = self.host()?;
            let _admission = host.lock_admission().await;
            let _parent = request.parent.admit().map_err(message)?;
            if host.shutdown.is_cancelled() {
                return Err("Host is draining".into());
            }
            let session = host
                .log
                .get_session::<SessionConfiguration>(&request.session)
                .await
                .map_err(message)?
                .ok_or("Session does not exist")?;
            if session.archived {
                return Err("Session is archived".into());
            }
            if request.previous.is_some()
                && let Some(owner) = host.active_session_owner(&request.session)
                && host
                    .log
                    .run_boundary(&request.session, &owner.run_id)
                    .await
                    .map_err(message)?
                    .is_some_and(|boundary| {
                        !matches!(
                            boundary.state,
                            maka_event_log::turns::InvocationState::Ended { .. }
                        )
                    })
            {
                return Err("Previous Graph supervisor is still settling".into());
            }
            let epoch = host
                .log
                .open_graph(
                    &request.session,
                    request.mode,
                    request.previous.as_ref(),
                    crate::plugins::graph::now()?,
                )
                .await
                .map_err(message)?;
            if host
                .log
                .graph_control(&request.session, Some(&epoch.graph_id))
                .await
                .map_err(message)?
                .is_some_and(|control| control.closed())
            {
                request.stop.cancel();
            }
            let context = request.child.context();
            let commands = host
                .authorize_plugin(
                    context.clone(),
                    &[request.session],
                    &self.root,
                    request.stop,
                )
                .await
                .map_err(message)?;
            let storage = host.plugin_store(context).map_err(message)?;
            let (handle, staged) = (request.build)(Opened {
                epoch,
                commands,
                storage,
            })?;
            request.child.ready().map_err(message)?;
            let lifecycle = host
                .plugin_catalog
                .publish_child(&request.parent, request.child, staged)
                .map_err(message)?;
            Ok(Root {
                lifecycle,
                handle,
                mode: request.mode,
            })
        })
    }
    fn retire_idle(
        &self,
        session: String,
        owner: Option<Context>,
    ) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async move {
            let host = self.host()?;
            let _admission = host.lock_admission().await;
            if host.has_session_work(&session).await.map_err(message)? {
                return Ok(false);
            }
            if let Some(owner) = owner {
                owner.retire();
            }
            Ok(true)
        })
    }
    fn stop(
        &self,
        session: String,
        graph: GraphId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let host = self.host().map_err(|_| Error::Retired)?;
            let _admission = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(Error::Cancelled),
                guard = host.lock_admission() => guard,
            };
            if cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let owner = host.active_session_owner(&session);
            let submission = host
                .plugin_catalog
                .snapshot::<Submission>(&Scope::Session(session.clone()))
                .entries
                .remove("agent-graph")
                .filter(|entry| entry.value.graph_id == graph);
            host.log
                .stop_graph(&session, &graph)
                .await
                .map_err(|error| {
                    if matches!(
                        error,
                        maka_event_log::StoreError::CommitUnknown(_)
                            | maka_event_log::StoreError::OperationUnknown
                    ) {
                        host.begin_drain();
                    }
                    Error::Provider(error.to_string())
                })?;
            if let Some(submission) = submission {
                submission.value.stop.cancel();
            }
            if let Some(owner) = owner {
                host.retire_owner(&owner, maka_agent::CancellationCause::Runtime)
                    .await
                    .map_err(|error| Error::Provider(error.message))?;
            }
            Ok(())
        })
    }
    fn preferences(&self) -> BoxFuture<'_, Result<Preferences, String>> {
        Box::pin(async move {
            let host = self.host()?;
            Ok(Preferences {
                presets: host
                    .configuration
                    .runtime_policy()
                    .await
                    .map_err(message)?
                    .policy
                    .subagents
                    .presets,
                models: host.configuration.catalog().await.map_err(message)?,
            })
        })
    }
}
fn message(error: impl ToString) -> String {
    error.to_string()
}
