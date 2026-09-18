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
    coordinator::Coordinator,
    decision::Decision,
    schedule::{Source, Update},
    view::Snapshot,
};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Clone)]
pub struct Handle {
    commands: mpsc::Sender<Command>,
    view: watch::Receiver<Arc<View>>,
}
pub struct View {
    pub swarm: Option<crate::swarm::Snapshot>,
    pub change_key: String,
    pub snapshot: Snapshot,
    pub error: Option<String>,
    pub initialized: bool,
    pub pending_work: bool,
}
struct Command {
    update: Box<Update>,
    reply: oneshot::Sender<Result<u64, Error>>,
}
impl Handle {
    pub fn snapshot(&self) -> Arc<View> {
        self.view.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<Arc<View>> {
        self.view.clone()
    }

    pub async fn update(&self, decision: Decision, source: Source) -> Result<u64, Error> {
        let snapshot = self.snapshot();
        if source.invocation.session_id != snapshot.snapshot.root_session_id {
            return Err(Error::Invalid(
                "tool source belongs to another graph root".into(),
            ));
        }
        let update = decision.update(snapshot.snapshot.graph_id.clone(), source)?;
        let (reply, receive) = oneshot::channel();
        self.commands
            .send(Command {
                update: Box::new(update),
                reply,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receive
            .await
            .map_err(|_| Error::Persistence("accepted graph decision outcome is unknown".into()))?
    }
}

pub fn start(
    mut coordinator: Coordinator,
) -> Result<
    (
        Handle,
        impl Future<Output = Result<(), String>> + Send + 'static,
    ),
    Error,
> {
    let mut changed = coordinator.changes()?;
    let (updates, view) = watch::channel(Arc::new(View {
        swarm: coordinator.swarm_snapshot(),
        change_key: coordinator.change_key(),
        snapshot: coordinator.snapshot(),
        error: None,
        initialized: false,
        pending_work: true,
    }));
    let (commands, mut requests) = mpsc::channel::<Command>(32);
    let handle = Handle { commands, view };
    let owner = async move {
        let mut retry = Duration::from_millis(250);
        loop {
            {
                // A fresh control request may preempt reads or a lost Host reply;
                // accepted work remains keyed by its already-persisted intent.
                enum Next {
                    Request(Option<Command>),
                    Reconciled(Result<(), Error>),
                }
                let next = tokio::select! {
                    biased;
                    command = requests.recv() => Next::Request(command),
                    result = coordinator.reconcile() => Next::Reconciled(result),
                };
                match next {
                    Next::Request(None) => return Ok(()),
                    Next::Request(Some(command)) => {
                        let result = coordinator.decide(*command.update, now()?).await;
                        let _ = command.reply.send(result);
                        continue;
                    }
                    Next::Reconciled(result) => {
                        // Session preparation may hold Host admission while it
                        // waits for this read model. Publish before a supervisor
                        // wake tries to acquire that same admission.
                        updates.send_replace(Arc::new(View {
                            swarm: coordinator.swarm_snapshot(),
                            change_key: coordinator.change_key(),
                            snapshot: coordinator.snapshot(),
                            error: result.as_ref().err().map(ToString::to_string),
                            initialized: true,
                            pending_work: coordinator.has_background_work(),
                        }));
                        let result = match result {
                            Ok(()) => coordinator.wake_supervisor().await,
                            Err(error) => Err(error),
                        };
                        let error = result.err().map(|error| error.to_string());
                        let retrying = error.is_some()
                            || !coordinator.failures.is_empty()
                            || coordinator.catching_up();
                        updates.send_replace(Arc::new(View {
                            swarm: coordinator.swarm_snapshot(),
                            change_key: coordinator.change_key(),
                            snapshot: coordinator.snapshot(),
                            error,
                            initialized: true,
                            pending_work: coordinator.has_background_work(),
                        }));
                        if retrying {
                            retry = (retry * 2).min(Duration::from_secs(30));
                        } else {
                            retry = Duration::from_millis(250);
                        }
                    }
                }
            }
            tokio::select! {
                biased;
                command = requests.recv() => match command {
                    None => return Ok(()),
                    Some(command) => {
                        let result = coordinator.decide(*command.update, now()?).await;
                        let _ = command.reply.send(result);
                    }
                },
                result = changed.changed() => {
                    result.map_err(|_| "Host execution observer closed".to_string())?;
                    // Coalesce token commits without placing one task per event.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    changed.borrow_and_update();
                },
                _ = tokio::time::sleep(retry), if coordinator.catching_up()
                    || updates.borrow().error.is_some() || !coordinator.failures.is_empty() => {}
            }
        }
    };
    Ok((handle, owner))
}
fn now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "system clock overflow".into())
}
