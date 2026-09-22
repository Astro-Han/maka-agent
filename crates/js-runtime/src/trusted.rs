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

//! One lazily started, trusted model isolate per Host. Network waits are
//! multiplexed; generated Code Mode JavaScript never enters it.
mod budget;
mod engine;
mod http;
mod lifetime;
mod ops;
mod responses;
mod responses_ops;
mod worker;
pub use responses::ResponsesLane;

use lifetime::{Health, ModelCancellation};
use serde_json::Value;
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, thiserror::Error)]
pub enum TrustedError {
    #[error("model request cancelled")]
    Cancelled,
    #[error("model stream idle timeout exceeded")]
    TimedOut,
    #[error("trusted JavaScript runtime failed: {0}")]
    Failed(String),
}

pub(super) type Result<T> = std::result::Result<T, TrustedError>;
pub(super) type Reply = oneshot::Sender<Result<()>>;

/// Queued SDK output retains its share of the runtime-wide byte budget until
/// the consumer takes it. Queue count alone is not a useful memory boundary
/// when model events vary from a few bytes to several MiB.
pub struct ProviderEvent {
    value: Value,
    _budget: OwnedSemaphorePermit,
}

impl ProviderEvent {
    pub fn into_value(self) -> Value {
        self.value
    }
}

#[derive(Clone)]
pub struct TrustedRuntime(Arc<Inner>);

struct Inner {
    service: OnceLock<Result<Service>>,
    slots: Arc<Semaphore>,
    input: Arc<Semaphore>,
    responses: Arc<responses::Shared>,
    sequence: AtomicU32,
}

struct Service {
    // The queue is bounded by 128 live requests: one start and one cancel each.
    commands: mpsc::UnboundedSender<Command>,
    health: Arc<Health>,
}

pub(super) enum Command {
    Model {
        id: u32,
        request: Value,
        sender: mpsc::Sender<Result<ProviderEvent>>,
        activity: Arc<Notify>,
        cancellation: CancellationToken,
        lane: Option<ResponsesLane>,
        network: maka_network::Policy,
        responses: Arc<responses::Shared>,
        permit: OwnedSemaphorePermit,
        input: OwnedSemaphorePermit,
        reply: Reply,
    },
    Cancel(u32),
}

impl Default for TrustedRuntime {
    fn default() -> Self {
        Self(Arc::new(Inner {
            service: OnceLock::new(),
            slots: Arc::new(Semaphore::new(128)),
            input: Arc::new(Semaphore::new(32 * 1024 * 1024)),
            responses: Arc::new(responses::Shared::default()),
            sequence: AtomicU32::new(1),
        }))
    }
}

impl TrustedRuntime {
    fn service(&self) -> Result<&Service> {
        self.0
            .service
            .get_or_init(|| {
                super::initialize_platform();
                let (commands, receiver) = mpsc::unbounded_channel();
                let health = Arc::new(Health::default());
                let watch = health.clone();
                std::thread::Builder::new()
                    .name("maka-trusted-js".into())
                    .spawn(move || {
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let scheduler = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .map_err(failed)?;
                            scheduler.block_on(worker::run(receiver, watch.clone()))
                        }));
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => watch.fail(error.to_string()),
                            Err(_) => {
                                watch.fail("worker panicked; active operations were not replayed")
                            }
                        }
                    })
                    .map_err(failed)?;
                Ok(Service { commands, health })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    fn next_id(&self) -> Result<u32> {
        self.0
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| failed("object id space exhausted"))
    }

    pub async fn model(
        &self,
        request: Value,
        sender: mpsc::Sender<Result<ProviderEvent>>,
        cancellation: CancellationToken,
        idle_timeout: Duration,
    ) -> Result<()> {
        self.model_in_lane(
            request,
            sender,
            cancellation,
            idle_timeout,
            None,
            Default::default(),
        )
        .await
    }

    pub async fn model_in_lane(
        &self,
        request: Value,
        sender: mpsc::Sender<Result<ProviderEvent>>,
        cancellation: CancellationToken,
        idle_timeout: Duration,
        lane: Option<ResponsesLane>,
        network: maka_network::Policy,
    ) -> Result<()> {
        let bytes = budget::bytes(&request, 32 * 1024 * 1024)?;
        let permit = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(TrustedError::Cancelled),
            permit = self.0.slots.clone().acquire_owned() => permit.map_err(failed)?,
        };
        // Large image/context requests queue before V8 conversion. Their
        // serialized-byte reservation lasts through request cleanup.
        let input = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(TrustedError::Cancelled),
            input = self.0.input.clone().acquire_many_owned(bytes) => input.map_err(failed)?,
        };
        let service = self.service()?;
        let id = self.next_id()?;
        let (reply, mut done) = oneshot::channel();
        let activity = Arc::new(Notify::new());
        service
            .commands
            .send(Command::Model {
                id,
                request,
                sender,
                activity: activity.clone(),
                cancellation: cancellation.clone(),
                lane,
                network,
                responses: self.0.responses.clone(),
                permit,
                input,
                reply,
            })
            .map_err(|_| service.health.error())?;
        let mut cancel_on_drop =
            ModelCancellation::new(id, service.commands.clone(), cancellation.clone());
        let cause = loop {
            tokio::select! {
                biased;
                result = &mut done => {
                    cancel_on_drop.disarm();
                    let result = result.map_err(|_| service.health.error())?;
                    return if cancellation.is_cancelled() { Err(TrustedError::Cancelled) } else { result };
                },
                _ = cancellation.cancelled() => break TrustedError::Cancelled,
                _ = activity.notified() => {},
                _ = tokio::time::sleep(idle_timeout) => break TrustedError::TimedOut,
            }
        };
        cancel_on_drop.cancel();
        // Abort is request-local. Only failure to settle cleanup escalates to a
        // fatal shared-runtime failure, never silently restarts/replays work.
        if tokio::time::timeout(Duration::from_secs(5), &mut done)
            .await
            .is_err()
        {
            service
                .health
                .fail("model cancellation failed to settle within 5 seconds");
            let _ = done.await;
        }
        Err(cause)
    }
}

pub(super) fn failed(error: impl std::fmt::Display) -> TrustedError {
    TrustedError::Failed(error.to_string())
}
