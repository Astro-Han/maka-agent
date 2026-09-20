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

mod dispatch;

use crate::{
    Error,
    authorization::Origin,
    command::{Mutation, MutationResult, Query, QueryResult},
    controller::Controller,
    delivery::{Clock, Dispatcher},
    view::View,
};
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{
    sync::{Notify, mpsc, oneshot, watch},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Handle {
    commands: mpsc::Sender<Command>,
    view: watch::Receiver<Arc<View>>,
    wake: Arc<Notify>,
}
struct Command {
    mutation: Mutation,
    origin: Origin,
    reply: oneshot::Sender<Result<MutationResult, Error>>,
}
impl Handle {
    pub fn snapshot(&self) -> Arc<View> {
        self.view.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<Arc<View>> {
        self.view.clone()
    }
    pub fn query(&self, query: Query) -> Result<QueryResult, Error> {
        if self.commands.is_closed() {
            return Err(Error::Closed);
        }
        self.view.borrow().query(query)
    }
    /// Called by Host's existing resume/wake notifications.
    pub fn wake(&self) {
        self.wake.notify_one();
    }
    pub async fn mutate(
        &self,
        mutation: Mutation,
        origin: Origin,
    ) -> Result<MutationResult, Error> {
        let (reply, receive) = oneshot::channel();
        self.commands
            .try_send(Command {
                mutation,
                origin,
                reply,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Error::Busy,
                mpsc::error::TrySendError::Closed(_) => Error::Closed,
            })?;
        // A timeout does not roll back a mutation already accepted by the owner.
        tokio::time::timeout(Duration::from_secs(10), receive)
            .await
            .map_err(|_| Error::OutcomeUnknown)?
            .map_err(|_| Error::OutcomeUnknown)?
    }
}
impl maka_plugins::background::BackgroundWork for Handle {
    fn wake(&self) {
        Handle::wake(self);
    }

    fn is_pending(&self) -> bool {
        let view = self.view.borrow();
        !self.commands.is_closed() && (!view.ready || view.pending_work)
    }
}

struct Owner {
    controller: Controller,
    dispatcher: Arc<dyn Dispatcher>,
    clock: Arc<dyn Clock>,
    updates: watch::Sender<Arc<View>>,
    jobs: FuturesUnordered<dispatch::Job>,
    active: BTreeMap<String, CancellationToken>,
    retries: BTreeMap<String, dispatch::Retry>,
}

/// The returned future belongs to the Fiber and must start only after publication.
/// Dropping it stops new triggers; Host-owned accepted executions are not cancelled.
pub fn start(
    controller: Controller,
    dispatcher: Arc<dyn Dispatcher>,
    clock: Arc<dyn Clock>,
    stop: CancellationToken,
) -> (
    Handle,
    impl Future<Output = Result<(), String>> + Send + 'static,
) {
    let (updates, view) = watch::channel(Arc::new(View::default()));
    let (commands, mut requests) = mpsc::channel::<Command>(32);
    let wake = Arc::new(Notify::new());
    let handle = Handle {
        commands,
        view,
        wake: wake.clone(),
    };
    let mut owner = Owner {
        controller,
        dispatcher,
        clock,
        updates,
        jobs: FuturesUnordered::new(),
        active: BTreeMap::new(),
        retries: BTreeMap::new(),
    };
    let future = async move {
        let mut last_tick = owner.clock.now();
        let mut reload = true;
        let mut recover = true;
        loop {
            if stop.is_cancelled() {
                return Ok(());
            }
            if reload {
                match owner.controller.reload().await {
                    Ok(()) => {
                        // A lost settlement acknowledgement can leave a retry in
                        // memory after its Fire has already committed as complete.
                        owner.reconcile_attempts();
                        reload = false;
                    }
                    Err(error) => {
                        owner.failed(error);
                        tokio::select! {
                            _ = stop.cancelled() => return Ok(()),
                            _ = tokio::time::sleep(Duration::from_secs(1)) => continue,
                        }
                    }
                }
            }
            let now = owner.clock.now();
            // A resumed machine or large forward clock adjustment follows the
            // configured misfire policy, not a backlog of timer callbacks.
            recover |= now.saturating_sub(last_tick) > 60_000;
            last_tick = now;
            if recover {
                if let Err(error) = owner.controller.recover(now).await {
                    owner.failed(error);
                    reload = true;
                } else {
                    recover = false;
                }
            }
            if !reload && let Err(error) = owner.launch(now).await {
                owner.failed(error);
                reload = true;
            }
            if !reload {
                owner.publish();
            }
            let delay = if reload {
                Duration::from_secs(1)
            } else {
                owner.delay(now)
            };
            tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                Some(completed) = owner.jobs.next(), if !owner.jobs.is_empty() => {
                    owner.active.remove(&completed.task_id);
                    if let Err(error) = owner.finish(completed).await {
                        owner.failed(error);
                        reload = true;
                    }
                }
                command = requests.recv() => {
                    let Some(command) = command else { return Ok(()); };
                    if command.reply.is_closed() { continue; }
                    if reload {
                        let _ = command.reply.send(Err(Error::Unavailable("scheduler is recovering".into())));
                        continue;
                    }
                    let result = owner.controller.mutate(command.mutation, command.origin, owner.clock.now(), owner.dispatcher.as_ref()).await;
                    if matches!(result, Err(Error::Storage(_))) {
                        owner.failed(result.as_ref().err().unwrap());
                        reload = true;
                    } else if result.is_ok() {
                        owner.reconcile_attempts();
                        owner.publish();
                    }
                    let _ = command.reply.send(result);
                }
                _ = wake.notified() => { recover = true; }
                _ = tokio::time::sleep(delay) => {}
            }
        }
    };
    (handle, future)
}
impl Owner {
    fn reconcile_attempts(&mut self) {
        // Domain state owns withdrawal. Host only observes the attempt's
        // cancellation signal, never the task catalog or plugin publication.
        for (id, stop) in &self.active {
            let active = self.controller.catalog.plans.get(id).is_some_and(|saved| {
                saved.plan.task.status == crate::task::Status::Active
                    && saved.plan.pending.is_some()
            });
            if !active {
                stop.cancel();
            }
        }
        self.retries.retain(|id, retry| {
            self.controller
                .catalog
                .plans
                .get(id)
                .and_then(|saved| saved.plan.pending.as_ref())
                .is_some_and(|fire| fire.id == retry.fire_id)
        });
    }
    fn publish(&self) {
        let previous = self.updates.borrow().clone();
        if !previous.ready || previous.revision != self.controller.catalog.revision.unwrap_or(0) {
            self.updates
                .send_replace(Arc::new(self.controller.view(&previous)));
        }
    }
    fn failed(&self, error: impl ToString) {
        // A lost mutation acknowledgement may hide a committed pause. Fence
        // unadmitted notifications until durable state has been reloaded.
        for stop in self.active.values() {
            stop.cancel();
        }
        let previous = self.updates.borrow().clone();
        self.updates.send_replace(Arc::new(View {
            revision: previous.revision,
            tasks: previous.tasks.clone(),
            ready: false,
            pending_work: previous.pending_work,
            error: Some(error.to_string()),
        }));
    }
    fn delay(&self, now: i64) -> Duration {
        // Recheck wall time periodically even without an OS wake notification.
        let mut delay = Duration::from_secs(30);
        if self.jobs.len() >= 8 {
            return delay;
        }
        for (id, saved) in &self.controller.catalog.plans {
            if self.active.contains_key(id) {
                continue;
            }
            if let Some(retry) = self.retries.get(id).filter(|retry| {
                saved
                    .plan
                    .pending
                    .as_ref()
                    .is_some_and(|fire| fire.id == retry.fire_id)
            }) {
                delay = delay.min(retry.at.saturating_duration_since(Instant::now()));
            } else if let Some(at) = saved.plan.task.next_fire_at
                && saved.plan.task.status == crate::task::Status::Active
            {
                delay = delay.min(Duration::from_millis(at.saturating_sub(now).max(0) as u64));
            } else if saved.plan.pending.is_some() {
                delay = Duration::ZERO;
            }
        }
        delay
    }
}
