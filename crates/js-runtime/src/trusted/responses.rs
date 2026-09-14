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

use deno_error::JsErrorBox;
use futures_util::{FutureExt, SinkExt, StreamExt};
use maka_network::{Policy, Socket};
use serde_json::Value;
use std::cell::Cell;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, JsErrorBox>;
const LIMIT: usize = 8 * 1024 * 1024;
mod cache;
mod connect;
mod frame;
mod output;
mod shared;
use cache::{Baseline, Prepared};
use frame::{ReadError, receive};
pub(super) use shared::Shared;

/// Ephemeral connection reuse, scoped to one sequential Turn. Clones are leases,
/// not history owners; dropping the final lease closes any idle socket.
#[derive(Clone, Default)]
pub struct ResponsesLane(Arc<Mutex<Lane>>);

#[derive(Default)]
struct Lane {
    idle: Option<Idle>,
    fallback: bool,
    busy: bool,
}
struct Idle {
    socket: Socket,
    url: String,
    headers: BTreeMap<String, String>,
    policy: Policy,
    baseline: Option<Baseline>,
    route: u64,
}

impl ResponsesLane {
    pub fn response_id(&self) -> Option<String> {
        let lane = self.0.lock().unwrap();
        if lane.busy || lane.fallback {
            return None;
        }
        lane.idle.as_ref()?.baseline.as_ref()?.response_id.clone()
    }
}

pub(super) struct Exchange {
    lane: ResponsesLane,
    active: AsyncMutex<Option<Idle>>,
    cancellation: CancellationToken,
    owns_lane: Cell<bool>,
    policy: Policy,
    transport: Arc<Shared>,
}

impl Exchange {
    pub fn new(
        lane: ResponsesLane,
        cancellation: CancellationToken,
        policy: Policy,
        transport: Arc<Shared>,
    ) -> Self {
        Self {
            lane,
            active: AsyncMutex::new(None),
            cancellation: cancellation.child_token(),
            owns_lane: Cell::new(false),
            policy,
            transport,
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub async fn start(
        &self,
        url: String,
        headers: BTreeMap<String, String>,
        body: Value,
    ) -> Result<Option<Value>> {
        // Cancellation drops the in-flight connect/send future and its socket.
        tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(error("Responses WebSocket cancelled")),
            result = self.start_inner(url, headers, body) => result,
        }
    }

    async fn start_inner(
        &self,
        url: String,
        headers: BTreeMap<String, String>,
        body: Value,
    ) -> Result<Option<Value>> {
        let route = self.transport.route(&url, &headers, &self.policy);
        let mut active = self.active.lock().await;
        if active.is_some() {
            return Err(error("Responses WebSocket request already active"));
        }
        let mut idle = {
            let mut lane = self.lane.0.lock().unwrap();
            if lane.fallback || lane.busy {
                // No caller may submit a shortened request without owning the
                // matching baseline. Ordinary concurrent full requests use HTTP.
                return Ok(Some(Prepared::new(body, None)?.full));
            }
            lane.busy = true;
            self.owns_lane.set(true);
            lane.idle.take()
        };
        // Reconstruct before rejecting an old socket: store:false state cannot
        // be recovered by carrying its response id to another connection.
        let prepared = Prepared::new(body, idle.as_mut().and_then(|idle| idle.baseline.take()))?;
        let mut idle = idle.filter(|idle| {
            idle.url == url && idle.headers == headers && idle.policy == self.policy
        });
        // Consume an already observable idle close before deciding to reuse.
        // After send starts, failure is never retried by this transport.
        if let Some(connection) = idle.as_mut() {
            loop {
                match connection.socket.next().now_or_never() {
                    None => break,
                    Some(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => continue,
                    _ => {
                        idle = None;
                        break;
                    }
                }
            }
        }
        let reuse = idle.is_some();
        if !reuse && self.transport.deferred(route, Instant::now()) {
            self.lane.0.lock().unwrap().busy = false;
            self.owns_lane.set(false);
            return Ok(Some(prepared.full));
        }
        let mut connection = match idle {
            Some(idle) => idle,
            None => match connect::open(&self.policy, &url, &headers).await? {
                Some(socket) => Idle {
                    socket,
                    url,
                    headers,
                    policy: self.policy.clone(),
                    baseline: None,
                    route,
                },
                _ => {
                    if !self.cancellation.is_cancelled() {
                        self.transport.defer(route, Instant::now());
                    }
                    let mut lane = self.lane.0.lock().unwrap();
                    lane.fallback = true;
                    lane.busy = false;
                    self.owns_lane.set(false);
                    return Ok(Some(prepared.full));
                }
            },
        };
        let body = if reuse {
            prepared.delta.as_ref().unwrap_or(&prepared.full)
        } else {
            &prepared.full
        };
        // Serialize a borrowed envelope without cloning the complete input.
        let mut object: BTreeMap<_, _> = body
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "stream" | "background"))
            .map(|(key, value)| (key.as_str(), value))
            .collect();
        let kind = Value::String("response.create".into());
        object.insert("type", &kind);
        let text =
            serde_json::to_string(&object).map_err(|_| error("invalid Responses request"))?;
        connection
            .socket
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| {
                if !self.cancellation.is_cancelled() {
                    self.transport.defer(route, Instant::now());
                }
                error("Responses WebSocket failed after dispatch; request was not replayed")
            })?;
        connection.baseline = prepared.cache(&self.transport.cache);
        *active = Some(connection);
        Ok(None)
    }

    pub async fn next(&self) -> Result<Option<String>> {
        let mut active = self.active.lock().await;
        let Some(connection) = active.as_mut() else {
            return Ok(None);
        };
        let result = tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Err(ReadError::Cancelled),
            result = receive(&mut connection.socket) => result,
        };
        match result {
            Ok((text, terminal, reusable, mut event)) => {
                connection.baseline = connection.baseline.take().and_then(|mut baseline| {
                    baseline.observe(&mut event, &self.transport.cache)?;
                    Some(baseline)
                });
                if terminal {
                    let mut idle = active.take();
                    if let Some(connection) = idle.as_mut() {
                        connection.baseline = connection.baseline.take().and_then(|baseline| {
                            baseline.complete(&mut event["response"], &self.transport.cache)
                        });
                    }
                    let mut lane = self.lane.0.lock().unwrap();
                    lane.busy = false;
                    self.owns_lane.set(false);
                    if reusable {
                        lane.idle = idle;
                    }
                }
                Ok(Some(text))
            }
            Err(error) => {
                if matches!(error, ReadError::Transport(_)) && !self.cancellation.is_cancelled() {
                    self.transport.defer(connection.route, Instant::now());
                }
                active.take();
                let mut lane = self.lane.0.lock().unwrap();
                lane.busy = false;
                lane.fallback = true;
                self.owns_lane.set(false);
                Err(error.into_js())
            }
        }
    }
}

impl Drop for Exchange {
    fn drop(&mut self) {
        if self.owns_lane.get() {
            let mut lane = self.lane.0.lock().unwrap();
            if lane.busy {
                lane.busy = false;
                lane.fallback = true;
                lane.idle.take();
            }
        }
    }
}

fn error(message: &'static str) -> JsErrorBox {
    JsErrorBox::generic(message)
}
