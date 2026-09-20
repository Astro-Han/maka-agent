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

//! Public callable Services. Native peers keep typed arguments; language
//! adapters use the same endpoint with a checked serialization boundary.

use super::{BoundServices, Service, ServiceView};
use crate::{Registration, call::Scope, fiber};
use futures_util::future::BoxFuture;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{any::Any, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Context {
    pub configuration: Vec<Value>,
    pub cancellation: CancellationToken,
    /// Forwarded Host call ownership, never reconstructed from a JSON identity.
    pub invocation: Option<Scope>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("service registration is retired")]
    Retired,
    #[error("invalid service input or result: {0}")]
    Invalid(String),
    #[error("service provider failed: {0}")]
    Failed(String),
    #[error("service outcome is unknown: {0}")]
    OutcomeUnknown(String),
}

pub trait Method<I, O>: Send + Sync {
    fn call(&self, input: I, context: Context) -> BoxFuture<'_, Result<O, Error>>;
}

struct Typed<I, O>(Arc<dyn Method<I, O>>);

trait Wire: Send + Sync {
    fn call(&self, input: Value, context: Context) -> BoxFuture<'_, Result<Value, Error>>;
}
impl<I, O> Wire for Typed<I, O>
where
    I: DeserializeOwned + Send + 'static,
    O: Serialize + Send + 'static,
{
    fn call(&self, input: Value, context: Context) -> BoxFuture<'_, Result<Value, Error>> {
        Box::pin(async move {
            let input = serde_json::from_value(input).map_err(invalid)?;
            let output = self.0.call(input, context).await?;
            serde_json::to_value(output).map_err(invalid)
        })
    }
}

pub struct Endpoint {
    native: Arc<dyn Any + Send + Sync>,
    wire: Arc<dyn Wire>,
}
impl Endpoint {
    pub fn new<I, O>(method: Arc<dyn Method<I, O>>) -> Self
    where
        I: DeserializeOwned + Send + 'static,
        O: Serialize + Send + 'static,
    {
        let typed = Arc::new(Typed(method));
        Self {
            native: typed.clone(),
            wire: typed,
        }
    }

    async fn call<I, O>(&self, input: I, context: Context) -> Result<O, Error>
    where
        I: Serialize + Send + 'static,
        O: DeserializeOwned + Send + 'static,
    {
        if let Some(native) = self.native.downcast_ref::<Typed<I, O>>() {
            return native.0.call(input, context).await;
        }
        let input = serde_json::to_value(input).map_err(invalid)?;
        let output = self.wire.call(input, context).await?;
        serde_json::from_value(output).map_err(invalid)
    }
}

/// Bound to one registration. Replacement never redirects an existing handle.
#[derive(Clone)]
pub struct Handle {
    service: Service<Endpoint>,
    configuration: Vec<Value>,
}
impl Handle {
    pub async fn call<I, O>(
        &self,
        input: I,
        invocation: Option<Scope>,
        cancellation: CancellationToken,
    ) -> Result<O, Error>
    where
        I: Serialize + Send + 'static,
        O: DeserializeOwned + Send + 'static,
    {
        if cancellation.is_cancelled()
            || invocation
                .as_ref()
                .is_some_and(|call| call.cancellation.is_cancelled())
        {
            return Err(Error::Retired);
        }
        let service = self.service.acquire().map_err(|_| Error::Retired)?;
        let consumer_stop = self
            .service
            .consumer
            .as_ref()
            .map(fiber::Context::stopping)
            .transpose()
            .map_err(|_| Error::Retired)?;
        let consumer_retired = consumer_stop.clone();
        let provider_stop = self
            .service
            .provider
            .stopping()
            .map_err(|_| Error::Retired)?;
        let source_stop = invocation.as_ref().map(|call| call.cancellation.clone());
        let mut ticket = invocation
            .as_ref()
            .map(|call| call.resources.reserve())
            .transpose()
            .map_err(|error| Error::Failed(error.to_string()))?;
        let child = invocation
            .as_ref()
            .map(Scope::child)
            .transpose()
            .map_err(|error| Error::Failed(error.to_string()))?;
        let cancellation = cancellation.child_token();
        let abandoned = cancellation.clone().drop_guard();
        let context = Context {
            configuration: self.configuration.clone(),
            cancellation: cancellation.clone(),
            invocation: child.clone(),
        };
        let (send, mut receive) = tokio::sync::oneshot::channel();
        self.service
            .provider
            .spawn_resource("Service call", move |retiring| async move {
                if let Some(ticket) = &mut ticket {
                    ticket.start();
                }
                let source = async {
                    match &child {
                        Some(child) => child.cancellation.cancelled().await,
                        None => std::future::pending().await,
                    }
                };
                let stop = context.cancellation.clone();
                let request = service.call(input, context);
                tokio::pin!(request);
                let mut result = tokio::select! {
                    biased;
                    _ = retiring.cancelled() => { stop.cancel(); request.await },
                    _ = cancelled(consumer_retired.as_ref()) => { stop.cancel(); request.await },
                    _ = stop.cancelled() => request.await,
                    _ = source => { stop.cancel(); request.await },
                    result = &mut request => result,
                };
                if let Some(child) = &child
                    && let Err(error) = child.finish().await
                {
                    result = Err(Error::OutcomeUnknown(error.to_string()));
                }
                let settled = match &result {
                    Err(Error::OutcomeUnknown(error)) => Err(error.clone()),
                    _ => Ok(()),
                };
                if let Some(ticket) = ticket {
                    ticket.complete(settled.clone());
                }
                let _ = send.send(result);
                settled
            })
            .map_err(|_| Error::Retired)?;
        let stopping = async {
            let source = async {
                match source_stop {
                    Some(source) => source.cancelled().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = cancellation.cancelled() => {},
                _ = cancelled(consumer_stop.as_ref()) => {},
                _ = provider_stop.cancelled() => {},
                _ = source => {},
            }
        };
        let result = tokio::select! {
            result = &mut receive => result,
            _ = stopping => {
                cancellation.cancel();
                // Timeout abandons only the reply. The provider Fiber keeps
                // the worker, its admission lease and resources until cleanup.
                tokio::time::timeout(Duration::from_secs(5), &mut receive).await
                    .map_err(|_| Error::OutcomeUnknown("Service did not acknowledge cancellation".into()))?
            }
        };
        drop(abandoned);
        result.map_err(|_| Error::OutcomeUnknown("Service worker disappeared".into()))?
    }
}

impl ServiceView {
    pub fn provide_method<I, O>(
        &self,
        owner: &fiber::Context,
        name: &str,
        method: Arc<dyn Method<I, O>>,
    ) -> Result<(), crate::Error>
    where
        I: DeserializeOwned + Send + 'static,
        O: Serialize + Send + 'static,
    {
        self.register_method(owner, name, method)
            .map(Registration::retain)
    }

    pub fn register_method<I, O>(
        &self,
        owner: &fiber::Context,
        name: &str,
        method: Arc<dyn Method<I, O>>,
    ) -> Result<Registration, crate::Error>
    where
        I: DeserializeOwned + Send + 'static,
        O: Serialize + Send + 'static,
    {
        self.register(owner, name, Arc::new(Endpoint::new(method)))
    }

    pub fn method(&self, name: &str) -> Result<Option<Handle>, crate::Error> {
        Ok(self.get::<Endpoint>(name)?.map(|service| Handle {
            service,
            configuration: self.intercepts(name).to_vec(),
        }))
    }
}

impl BoundServices {
    pub fn provide_method<I, O>(
        &self,
        name: &str,
        method: Arc<dyn Method<I, O>>,
    ) -> Result<(), crate::Error>
    where
        I: DeserializeOwned + Send + 'static,
        O: Serialize + Send + 'static,
    {
        self.register_method(name, method).map(Registration::retain)
    }
    pub fn register_method<I, O>(
        &self,
        name: &str,
        method: Arc<dyn Method<I, O>>,
    ) -> Result<Registration, crate::Error>
    where
        I: DeserializeOwned + Send + 'static,
        O: Serialize + Send + 'static,
    {
        self.register(name, Arc::new(Endpoint::new(method)))
    }
    pub fn method(&self, name: &str) -> Result<Option<Handle>, crate::Error> {
        Ok(self.get::<Endpoint>(name)?.map(|service| Handle {
            service,
            configuration: self.view.intercepts(name).to_vec(),
        }))
    }
}

async fn cancelled(token: Option<&CancellationToken>) {
    match token {
        Some(token) => token.cancelled().await,
        None => std::future::pending().await,
    }
}

fn invalid(error: impl ToString) -> Error {
    Error::Invalid(error.to_string())
}
