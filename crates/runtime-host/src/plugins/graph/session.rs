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

use super::{ID, Manager, NativeOperators, Root, Submission, error, now, tools};
use crate::session::SessionConfiguration;
use futures_util::future::BoxFuture;
use maka_graph::{Mode, coordinator::Coordinator, owner::Handle};
use maka_plugins::{
    composition::Scope,
    fiber::Fiber,
    session::{Behavior, Preparation},
};
use maka_runtime::execution::OrchestrationMode;
use std::{sync::Arc, time::Duration};

impl Behavior for Manager {
    fn prepare(
        &self,
        session_id: String,
        orchestration: OrchestrationMode,
    ) -> BoxFuture<'_, Result<Preparation, String>> {
        Box::pin(async move {
            let mode = match orchestration {
                OrchestrationMode::Graph => Mode::Graph,
                OrchestrationMode::Swarm => Mode::Swarm,
                OrchestrationMode::Default => {
                    return Err("Session behavior requires Graph or Swarm".into());
                }
            };
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
            let submission = self
                .catalog
                .snapshot::<Submission>(&Scope::Session(session_id))
                .entries
                .remove("agent-graph")
                .filter(|entry| entry.value.graph_id == handle.snapshot().snapshot.graph_id)
                .ok_or("Graph submission authority was retired")?;
            Ok(Preparation {
                admission: Some(submission.value.stop.clone()),
                instructions: if handle.snapshot().swarm.is_some() {
                    "You are the Agent Swarm supervisor. Prefer parallel delegation when at least two meaningful independent items improve the work; handle small, conversational, latency-sensitive or indivisible requests directly. Do only lightweight exploration before delegation. Discover agent_list, agent_swarm_status, view_agent_graph, update_agent_graph and yield_agent_graph with tool_search. Use exact available agent/preset IDs, bounded self-contained instructions and non-overlapping writes. Schedule independent items together, then yield_agent_graph without polling, sleeping or watching child logs. Host wakes you only for attention or a settled batch. Inspect compact status with agent_swarm_status; read committed final results with view_agent_graph using work_id and record_id. Replace failed work using replaces. Yield if work remains active; otherwise finish the graph with selected result IDs, verify and synthesize the answer. Do not manufacture parallel busywork."
                } else {
                    "You are the Agent Graph supervisor. Prefer delegation when it materially improves the work; handle small or indivisible requests directly. Discover agent_list, view_agent_graph, update_agent_graph and yield_agent_graph with tool_search. Delegate bounded work using exact available agent/preset IDs, inspect durable results and schedule dependent work using record IDs. Avoid overlapping writes. When waiting, yield_agent_graph rather than poll; durable outcomes will wake you. Finish the graph by selecting result record IDs, then answer the user. A tool error is observable state: inspect before retrying a scheduling decision."
                }.into(),
                tool_ceiling: None,
            })
        })
    }
}
impl Manager {
    pub(super) async fn restore(&self) -> Result<(), String> {
        let mut after = None;
        let mut failure = None;
        loop {
            let roots = self
                .log
                .graph_roots(after.as_deref())
                .await
                .map_err(error)?;
            if roots.is_empty() {
                return failure.map_or(Ok(()), Err);
            }
            for root in &roots {
                let Some(session) = self
                    .log
                    .get_session::<SessionConfiguration>(root)
                    .await
                    .map_err(error)?
                else {
                    continue;
                };
                if !session.archived {
                    let control = self.log.graph_control(root, None).await.map_err(error)?;
                    if let Some(control) = control
                        && let Err(error) = self.ensure(root, control.epoch.mode, false).await
                    {
                        failure = Some(error);
                    }
                }
            }
            after = roots.last().cloned();
        }
    }

    async fn ensure(
        &self,
        session_id: &str,
        mode: Mode,
        reopen_closed: bool,
    ) -> Result<Handle, String> {
        let host = self.executions.upgrade().ok_or("Host is closed")?;
        let session = self
            .log
            .get_session::<SessionConfiguration>(session_id)
            .await
            .map_err(error)?
            .ok_or("Session does not exist")?;
        if session.archived {
            return Err("Session is archived".into());
        }
        let mut slot = self.lock_root(session_id).await?;
        // A cold slot must recover the old epoch before proving it quiescent.
        // The prepare loop can then CAS-roll a closed epoch to the requested mode.
        let mode = if slot.is_none() {
            match self
                .log
                .graph_control(session_id, None)
                .await
                .map_err(error)?
            {
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
            if root.lifecycle.is_effective()
                && (!reopen_closed || !view.initialized || !view.snapshot.closed)
            {
                return Ok(root.handle.clone());
            }
            if (view.snapshot.closed || root.mode != mode)
                && (!view.initialized || !view.snapshot.quiescent)
            {
                return Err("Previous Graph epoch is still settling".into());
            }
            if view.snapshot.closed {
                previous = Some(view.snapshot.graph_id.clone());
            }
            root.lifecycle
                .shutdown(tokio::time::Instant::now() + Duration::from_secs(10))
                .await
                .map_err(error)?;
        }
        // Environment preparation runs outside Host admission. Publish its
        // submission gate under that lock, so stop either sees the gate or
        // this initializer observes the durable stop before becoming effective.
        let _admission = host.lock_admission().await;
        if previous.is_some()
            && let Some(owner) = host.active_session_owner(session_id)
            && self
                .log
                .run_boundary(session_id, &owner.run_id)
                .await
                .map_err(error)?
                .is_some_and(|boundary| {
                    !matches!(
                        boundary.state,
                        maka_event_log::turns::InvocationState::Ended { .. }
                    )
                })
        {
            return Err("Previous Graph supervisor is still settling".into());
        }
        let epoch = self
            .log
            .open_graph(session_id, mode, previous.as_ref(), now()?)
            .await
            .map_err(error)?;
        let child = Fiber::new(
            ID,
            &format!("graph-{}", uuid::Uuid::new_v4().simple()),
            Scope::Session(session_id.into()),
        )
        .map_err(error)?;
        child.begin_loading().map_err(error)?;
        let context = child.context();
        let submission = Submission {
            graph_id: epoch.graph_id.clone(),
            stop: tokio_util::sync::CancellationToken::new(),
        };
        if self
            .log
            .graph_control(session_id, Some(&epoch.graph_id))
            .await
            .map_err(error)?
            .is_some_and(|control| control.closed())
        {
            submission.stop.cancel();
        }
        let commands = host
            .authorize_plugin(
                context.clone(),
                &[session_id.into()],
                &self.root,
                submission.stop.clone(),
            )
            .await
            .map_err(error)?;
        let operators = Arc::new(NativeOperators {
            commands: commands.clone(),
            log: self.log.clone(),
            context: context.clone(),
            root: session_id.into(),
            graph_id: epoch.graph_id.clone(),
            definitions: super::definitions::Definitions {
                configuration: self.configuration.clone(),
                catalog: self.catalog.clone(),
            },
            storage: host.plugin_store(context.clone()).map_err(error)?,
        });
        let coordinator = Coordinator::new(epoch, self.log.clone(), commands, operators.clone())
            .map_err(error)?;
        let (handle, owner) = maka_graph::owner::start(coordinator).map_err(error)?;
        context
            .spawn("Agent Graph coordinator", owner)
            .map_err(error)?;
        let mut staged = tools::register(handle.clone(), operators)?;
        staged
            .insert("agent-graph", handle.clone())
            .map_err(error)?;
        staged.insert("agent-graph", submission).map_err(error)?;
        child.ready().map_err(error)?;
        let lifecycle = self
            .catalog
            .publish_child(&self.parent, child, staged)
            .map_err(error)?;
        *slot = Some(Root {
            lifecycle,
            handle: handle.clone(),
            mode,
        });
        Ok(handle)
    }

    async fn lock_root(
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
            let host = self.executions.upgrade().ok_or("Host is closed")?;
            let admission = host.lock_admission().await;
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
                if idle && !host.has_session_work(&id).await.map_err(error)? {
                    if let Some(root) = guard.as_ref() {
                        root.lifecycle.retire();
                    }
                    retired = Some((id, slot, guard));
                    break;
                }
            }
            drop(admission);
            let Some((id, slot, guard)) = retired else {
                return Err("Agent Graph active Session limit reached".into());
            };
            // Keep the slot reserved through cleanup. Unknown cleanup must not
            // make room for a replacement; durable graph history is untouched.
            if let Some(root) = guard.as_ref() {
                root.lifecycle
                    .shutdown(tokio::time::Instant::now() + Duration::from_secs(10))
                    .await
                    .map_err(error)?;
            }
            let mut roots = self.roots.lock().unwrap();
            if roots
                .get(&id)
                .is_some_and(|current| Arc::ptr_eq(current, &slot))
            {
                roots.remove(&id);
            }
        }
    }
}
