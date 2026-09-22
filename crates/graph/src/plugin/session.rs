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

use super::{Manager, NativeOperators, Root, error};
use crate::{Mode, coordinator::Coordinator, owner::Handle};
use futures_util::{FutureExt, future::BoxFuture};
use maka_plugins::session::{Behavior, Preparation};
use std::{sync::Arc, time::Duration};

pub(super) struct GraphBehavior {
    pub manager: Arc<Manager>,
    pub mode: Mode,
}
impl Behavior for GraphBehavior {
    fn prepare(
        &self,
        request: maka_plugins::session::Request,
    ) -> BoxFuture<'_, Result<Preparation, String>> {
        self.manager.prepare(request.session.session_id, self.mode)
    }
}
impl Manager {
    fn prepare(
        &self,
        session_id: String,
        mode: Mode,
    ) -> BoxFuture<'_, Result<Preparation, String>> {
        Box::pin(async move {
            let mut handle = self.ensure(&session_id, mode, false).await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let mut changes = handle.subscribe();
                let view = tokio::time::timeout_at(deadline, async {
                    loop {
                        let view = changes.borrow_and_update().clone();
                        if view.initialized {
                            if let Some(error) = &view.error {
                                return Err(error.clone());
                            }
                            return Ok(view);
                        }
                        changes
                            .changed()
                            .await
                            .map_err(|_| "Graph coordinator stopped".to_owned())?;
                    }
                })
                .await
                .map_err(|_| "Graph recovery is still in progress".to_owned())??;
                if !view.snapshot.closed {
                    if view.swarm.is_some() != (mode == Mode::Swarm) {
                        return Err(
                            "Finish or stop the current Graph before changing its mode".into()
                        );
                    }
                    break;
                }
                handle = self.ensure(&session_id, mode, true).await?;
            }
            let slot = self.lock_root(&session_id).await?;
            let root = slot
                .as_ref()
                .filter(|root| {
                    root.handle.snapshot().snapshot.graph_id == handle.snapshot().snapshot.graph_id
                })
                .filter(|root| !root.cancel.is_cancelled())
                .ok_or("Graph submission authority was retired")?;
            let basis = root.revision.capture().await;
            Ok(Preparation {
                basis: Some(basis),
                instructions: if handle.snapshot().swarm.is_some() {
                    "You are the Agent Swarm supervisor. Prefer parallel delegation when at least two meaningful independent items improve the work; handle small, conversational, latency-sensitive or indivisible requests directly. Do only lightweight exploration before delegation. Discover agent_list, agent_swarm_status, view_agent_graph, update_agent_graph and yield_agent_graph with tool_search. Copy available targets from agent_list; use bounded self-contained instructions and non-overlapping writes. Schedule independent items together, then yield_agent_graph without polling, sleeping or watching child logs. Host wakes you only for attention or a settled batch. Inspect compact status with agent_swarm_status; read committed final results with view_agent_graph using work_id and record_id. Replace failed work using replaces. Yield if work remains active; otherwise finish the graph with selected result IDs, verify and synthesize the answer. Do not manufacture parallel busywork."
                } else {
                    "You are the Agent Graph supervisor. Prefer delegation when it materially improves the work; handle small or indivisible requests directly. Discover agent_list, view_agent_graph, update_agent_graph and yield_agent_graph with tool_search. Delegate bounded work by copying available targets from agent_list, inspect durable results and schedule dependent work using record IDs. Avoid overlapping writes. When waiting, yield_agent_graph rather than poll; durable outcomes will wake you. Finish the graph by selecting result record IDs, then answer the user. A tool error is observable state: inspect before retrying a scheduling decision."
                }.into(),
                tool_ceiling: None,
                ..Default::default()
            })
        })
    }
}
impl Manager {
    pub(super) async fn stop(
        &self,
        session: &str,
        graph: &crate::GraphId,
        commands: Arc<dyn maka_plugins::execution::Commands>,
    ) -> Result<(), maka_plugins::remote::Error> {
        use maka_plugins::remote::Error;
        let fail = |error: String| Error::Provider(error);
        let control = self
            .repository
            .current(session)
            .await
            .map_err(super::remote::graph_error)?
            .ok_or_else(|| Error::Invalid("Graph does not exist".into()))?;
        if control.epoch.graph_id != *graph {
            return Err(Error::Invalid("Graph epoch changed".into()));
        }
        let slot = self.lock_root(session).await.map_err(fail)?;
        let revision = slot
            .as_ref()
            .filter(|root| root.handle.snapshot().snapshot.graph_id == *graph)
            .map(|root| root.revision.clone());
        // Close prepared admissions before waiting on Host's admission gate.
        // An admission already in progress finishes first and is then observed.
        let _invalidating = match revision {
            Some(revision) => Some(revision.invalidate().await),
            None => None,
        };
        let activity = commands
            .activity(session.into())
            .await
            .map_err(|e| super::remote::graph_error(e.into()))?;
        let target = activity
            .execution
            .filter(|execution| {
                execution
                    .behavior
                    .as_ref()
                    .is_some_and(|behavior| matches!(behavior.as_str(), "graph" | "swarm"))
            })
            .map(|execution| execution.invocation);
        let control = self
            .repository
            .stop(session, graph, target)
            .await
            .map_err(super::remote::graph_error)?;
        if let Some(target) = control.stop_target {
            commands
                .stop(target)
                .await
                .map_err(|e| super::remote::graph_error(e.into()))?;
        }
        use crate::store::Store as _;
        let source = super::read::Source {
            repository: self.repository.as_ref(),
            storage: self.access.storage.as_ref(),
            commands: commands.as_ref(),
        };
        let mut after = 0;
        while after < control.schedule_revision {
            let page = self
                .repository
                .updates(graph, after, control.schedule_revision)
                .await
                .map_err(super::remote::graph_error)?;
            if page.is_empty() {
                return Err(Error::Provider("Graph schedule is incomplete".into()));
            }
            for update in page {
                after = update.revision;
                for work in update.update.add_work {
                    if let Some(intent) = self
                        .repository
                        .intent(graph, &work.work_id)
                        .await
                        .map_err(super::remote::graph_error)?
                        && source.observe(session, &intent).await?.is_some()
                    {
                        commands
                            .cancel(intent.request.operation_id)
                            .await
                            .map_err(|error| super::remote::graph_error(error.into()))?;
                    }
                }
            }
        }
        self.changed.send_replace(());
        Ok(())
    }

    pub(super) async fn restore(&self) -> Result<(), String> {
        let mut after = None;
        let mut failure = None;
        loop {
            let (roots, next) = self.repository.roots(after.take()).await.map_err(error)?;
            if roots.is_empty() {
                return failure.map_or(Ok(()), Err);
            }
            for root in &roots {
                match self.access.commands(root).await {
                    Ok(_) => {}
                    Err(crate::Error::Host(
                        maka_plugins::execution::CommandError::Denied
                        | maka_plugins::execution::CommandError::Revoked
                        | maka_plugins::execution::CommandError::NotFound,
                    )) => continue,
                    Err(error) => return Err(error.to_string()),
                }
                let control = self.repository.current(root).await.map_err(error)?;
                if let Some(control) = control
                    && let Err(error) = self.ensure(root, control.epoch.mode, false).await
                {
                    failure = Some(error);
                }
            }
            if next.is_none() {
                return failure.map_or(Ok(()), Err);
            }
            after = next;
        }
    }

    async fn ensure(
        &self,
        session_id: &str,
        mode: Mode,
        reopen_closed: bool,
    ) -> Result<Handle, String> {
        let commands = self.access.commands(session_id).await.map_err(error)?;
        let mut slot = self.lock_root(session_id).await?;
        // A cold slot must recover the old epoch before proving it quiescent.
        // The prepare loop can then CAS-roll a closed epoch to the requested mode.
        let mode = if slot.is_none() {
            match self.repository.current(session_id).await.map_err(error)? {
                Some(control) if control.epoch.mode != mode => {
                    if !control.closed() {
                        return Err(
                            "Finish or stop the current Graph before changing its mode".into()
                        );
                    }
                    control.epoch.mode
                }
                _ => mode,
            }
        } else {
            mode
        };
        let mut previous = None;
        if let Some(root) = slot.as_ref() {
            let view = root.handle.snapshot();
            if root.mode != mode && view.initialized && !view.snapshot.closed {
                return Err("Finish or stop the current Graph before changing its mode".into());
            }
            if !root.cancel.is_cancelled()
                && (!reopen_closed || !view.initialized || !view.snapshot.closed)
            {
                match root.commands.session(session_id.into()).await {
                    Ok(_) => return Ok(root.handle.clone()),
                    Err(
                        maka_plugins::execution::CommandError::Denied
                        | maka_plugins::execution::CommandError::Revoked,
                    ) => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
            if (view.snapshot.closed || root.mode != mode)
                && (!view.initialized || !view.snapshot.quiescent)
            {
                return Err("Previous Graph epoch is still settling".into());
            }
            if view.snapshot.closed {
                previous = Some(view.snapshot.graph_id.clone());
            }
            root.shutdown().await?;
        }
        if previous.is_some()
            && commands
                .activity(session_id.into())
                .await
                .map_err(error)?
                .execution
                .is_some_and(|execution| {
                    !matches!(
                        execution.progress,
                        maka_plugins::execution::Progress::Ended { .. }
                    )
                })
        {
            return Err("Previous Graph supervisor is still settling".into());
        }
        let epoch = self
            .repository
            .open(session_id, mode, previous.as_ref(), super::now()?)
            .await
            .map_err(error)?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let revision = maka_plugins::revision::Revision::default();
        let operators = Arc::new(NativeOperators {
            revision: revision.clone(),
            repository: self.repository.clone(),
            commands: commands.clone(),
            cancel: cancel.clone(),
            root: session_id.into(),
            graph_id: epoch.graph_id.clone(),
            definitions: super::definitions::Definitions {
                settings: self.settings.clone(),
                commands: commands.clone(),
                root: session_id.into(),
                models: self.models.clone(),
            },
            storage: self.access.storage.clone(),
        });
        let coordinator = Coordinator::new(
            epoch,
            self.repository.clone(),
            commands.clone(),
            operators.clone(),
        )
        .map_err(error)?;
        let (handle, owner) = crate::owner::start(coordinator).map_err(error)?;
        let stopping = cancel.clone();
        let done = self
            .parent
            .spawn("Agent Graph coordinator", async move {
                let _stopped = stopping.clone().drop_guard();
                tokio::select! {
                    biased;
                    _ = stopping.cancelled() => Ok(()),
                    result = owner => result,
                }
            })
            .map_err(error)?;
        let root = Root {
            revision,
            commands,
            cancel,
            done: async move { done.await.map_err(error)? }.boxed().shared(),
            operators,
            handle,
            mode,
        };
        let handle = root.handle.clone();
        *slot = Some(root);
        self.changed.send_replace(());
        Ok(handle)
    }

    pub(super) async fn lock_root(
        &self,
        session_id: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<Option<Root>>, String> {
        loop {
            let slot = {
                let mut roots = self.roots.lock().unwrap();
                if roots.contains_key(session_id) || roots.len() < 256 {
                    Some(roots.entry(session_id.into()).or_default().clone())
                } else {
                    None
                }
            };
            if let Some(slot) = slot {
                let guard = slot.clone().lock_owned().await;
                // A waiter may have captured a slot immediately before eviction.
                // Never initialize a second coordinator outside the current map.
                if self
                    .roots
                    .lock()
                    .unwrap()
                    .get(session_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &slot))
                {
                    return Ok(guard);
                }
                continue;
            }
            let candidates: Vec<_> = self
                .roots
                .lock()
                .unwrap()
                .iter()
                .map(|(id, slot)| (id.clone(), slot.clone()))
                .collect();
            let mut retired = None;
            for (id, slot) in candidates {
                let Ok(guard) = slot.clone().try_lock_owned() else {
                    continue;
                };
                let idle = guard.as_ref().is_none_or(|root| {
                    let view = root.handle.snapshot();
                    view.initialized
                        && view.error.is_none()
                        && view.snapshot.closed
                        && view.snapshot.quiescent
                        && !view.pending_work
                });
                let idle = if idle {
                    if let Some(root) = guard.as_ref() {
                        match root.commands.activity(id.clone()).await {
                            Ok(activity) => !activity.busy,
                            Err(
                                maka_plugins::execution::CommandError::Revoked
                                | maka_plugins::execution::CommandError::Denied
                                | maka_plugins::execution::CommandError::NotFound,
                            ) => true,
                            Err(error) => return Err(error.to_string()),
                        }
                    } else {
                        true
                    }
                } else {
                    false
                };
                if idle {
                    if let Some(root) = guard.as_ref() {
                        root.cancel.cancel();
                    }
                    retired = Some((id, slot, guard));
                    break;
                }
            }
            let Some((id, slot, guard)) = retired else {
                return Err("Agent Graph active Session limit reached".into());
            };
            // Keep the slot reserved through cleanup. Unknown cleanup must not
            // make room for a replacement; durable graph history is untouched.
            if let Some(root) = guard.as_ref() {
                root.shutdown().await?;
            }
            let mut roots = self.roots.lock().unwrap();
            if roots
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &slot))
            {
                roots.remove(&id);
                self.changed.send_replace(());
            }
        }
    }
}
