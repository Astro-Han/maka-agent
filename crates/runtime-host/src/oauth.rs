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

use maka_config::oauth::OAuthCredential;
use maka_model::{
    ModelError,
    oauth::{Client, Tokens},
};
use maka_runtime::oauth::Provider;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{Mutex as AsyncMutex, watch};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

mod settlement;
use settlement::State;

/// One root owns refresh coordination. A caller owns only its waiter; admitted
/// refresh and persistence jobs remain tracked until they settle.
pub struct Authority {
    entries: Mutex<HashMap<String, Arc<Generation>>>,
    workers: TaskTracker,
    shutdown: CancellationToken,
}

#[derive(Clone)]
pub struct Credential {
    generation: Arc<Generation>,
    revision: u64,
}

struct Generation {
    provider: Provider,
    identity: String,
    state: Arc<AsyncMutex<State>>,
    flight: Arc<Mutex<Option<RefreshWaiter>>>,
    workers: TaskTracker,
    shutdown: CancellationToken,
    superseded: CancellationToken,
}

#[derive(Clone)]
pub(crate) struct ResolvedToken {
    pub access_token: String,
    pub credential: OAuthCredential,
}

type RefreshWaiter = watch::Receiver<Option<Result<ResolvedToken, ModelError>>>;

impl Authority {
    pub fn new(workers: TaskTracker, shutdown: CancellationToken) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            workers,
            shutdown,
        }
    }

    /// The caller holds admission and has committed this exact deletion. Never
    /// infer deletion from absence in an asynchronously obtained catalog snapshot.
    pub(crate) fn forget_connection(&self, id: &str) {
        if let Some(generation) = self.entries.lock().unwrap().remove(id) {
            generation.superseded.cancel();
        }
    }

    /// Binding performs no network I/O and never waits for a running refresh.
    pub fn bind(
        &self,
        snapshot: OAuthCredential,
        provider: Provider,
    ) -> Result<Credential, ModelError> {
        if snapshot.target().provider_type != provider.as_str() {
            return Err(failure(
                "OAuth provider does not match credential authority",
            ));
        }
        let mut entries = self.entries.lock().unwrap();
        let id = &snapshot.target().connection_id;
        let revision = snapshot.basis().revision;
        if let Some(existing) = entries.get(id) {
            if existing.identity == snapshot.basis().credential_id && existing.provider == provider
            {
                match existing.state.try_lock() {
                    Err(_) => {
                        return Ok(Credential {
                            generation: existing.clone(),
                            revision,
                        });
                    }
                    Ok(state) if state.credential.basis() == snapshot.basis() => {
                        return Ok(Credential {
                            generation: existing.clone(),
                            revision,
                        });
                    }
                    Ok(state) if state.credential.basis().revision > snapshot.basis().revision => {
                        return Err(failure("OAuth binding snapshot is outdated"));
                    }
                    Ok(_) => {}
                }
            }
            existing.superseded.cancel();
        }
        let binding = Arc::new(Generation {
            provider,
            identity: snapshot.basis().credential_id.clone(),
            state: Arc::new(AsyncMutex::new(State {
                credential: snapshot,
                outcome: settlement::Outcome::Ready,
            })),
            flight: Arc::new(Mutex::new(None)),
            workers: self.workers.clone(),
            shutdown: self.shutdown.clone(),
            superseded: CancellationToken::new(),
        });
        entries.insert(
            binding
                .state
                .try_lock()
                .unwrap()
                .credential
                .target()
                .connection_id
                .clone(),
            binding.clone(),
        );
        Ok(Credential {
            generation: binding,
            revision,
        })
    }
}

impl Credential {
    /// The caller pins the transport per invocation. A shared flight may advance
    /// this binding, but must never return a token older than its admitted basis.
    pub async fn access_token(&self, client: Client) -> Result<String, ModelError> {
        Ok(self.resolve(client).await?.access_token)
    }

    pub(crate) async fn resolve(&self, client: Client) -> Result<ResolvedToken, ModelError> {
        let resolved = self.generation.resolve(client).await?;
        if resolved.credential.basis().revision < self.revision {
            return Err(failure("OAuth binding was superseded during resolution"));
        }
        Ok(resolved)
    }
}

impl Generation {
    async fn resolve(&self, client: Client) -> Result<ResolvedToken, ModelError> {
        if self.shutdown.is_cancelled() || self.superseded.is_cancelled() {
            return Err(ModelError::Cancelled);
        }
        let mut receiver = {
            let mut flight = self.flight.lock().unwrap();
            if let Some(receiver) = &*flight {
                receiver.clone()
            } else {
                let (sender, receiver) = watch::channel(None);
                *flight = Some(receiver.clone());
                let state = self.state.clone();
                let provider = self.provider;
                let shutdown = self.shutdown.clone();
                let superseded = self.superseded.clone();
                let flight = self.flight.clone();
                self.workers.spawn(async move {
                    let mut state = state.lock().await;
                    // A cancelled waiter is irrelevant once admitted. Root drain
                    // prevents new grants, never persistence of a spent grant.
                    let result = if superseded.is_cancelled() {
                        Err(failure("OAuth credential was superseded"))
                    } else {
                        state
                            .resolve(provider, client, &shutdown, &superseded)
                            .await
                    };
                    sender.send_replace(Some(result));
                    *flight.lock().unwrap() = None;
                });
                receiver
            }
        };
        let result = receiver
            .wait_for(Option::is_some)
            .await
            .map_err(|_| failure("OAuth owner stopped before settlement"))?
            .as_ref()
            .expect("settled result")
            .clone();
        if self.superseded.is_cancelled() {
            return Err(failure("OAuth credential was superseded"));
        }
        result
    }
}

fn failure(message: impl ToString) -> ModelError {
    ModelError::Adapter(message.to_string())
}
