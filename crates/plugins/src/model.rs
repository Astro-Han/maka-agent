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

//! Model protocol implementations are ordinary plugin contributions.
use crate::{contributions::Contribution, fiber::CallGuard, http};
use futures_util::future::BoxFuture;
pub use maka_runtime::model::{
    ModelEvent,
    error::ModelError as Error,
    prompt::Message,
    request::{Credentials, Provider, ProviderKind, Request},
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

/// Scoped I/O is supplied after Host model admission. Implementations cannot
/// replace routing policy or reconstruct this capability from a Session ID.
pub trait Transport: Send + Sync {
    /// Process-local routing identity, excluding the serialized credential.
    /// A changed routing policy must invalidate cached connections.
    fn identity(&self) -> u64;
    fn request(&self, request: http::Request) -> BoxFuture<'_, Result<http::Response, Error>>;
    fn connect(&self, request: Connect) -> BoxFuture<'_, Result<Arc<dyn Socket>, Error>>;
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Connect {
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}
/// One outstanding reader and writer. Cancelling a pending receive must not
/// consume the next frame; dropping the last handle closes the connection.
pub trait Socket: Send + Sync {
    fn send(&self, frame: Frame) -> BoxFuture<'_, Result<(), Error>>;
    fn receive(&self) -> BoxFuture<'_, Result<Option<Frame>, Error>>;
    fn close(&self) -> BoxFuture<'_, Result<(), Error>>;
}
pub trait Events: Send + Sync {
    /// Observed provider progress (including heartbeats and partial wire items)
    /// resets the idle deadline without inventing a canonical model event.
    fn progress(&self) {}
    /// Backpressure is observed before the next event is produced.
    fn emit(&self, event: ModelEvent) -> BoxFuture<'_, Result<(), Error>>;
}
#[derive(Clone)]
pub struct Context {
    pub transport: Arc<dyn Transport>,
    pub events: Arc<dyn Events>,
    pub cancellation: CancellationToken,
    pub idle_timeout: Duration,
}
/// Committed history projected by Host after local tool settlement. It is not
/// a plugin-supplied claim, nor permission to change canonical history.
#[derive(Clone, Serialize, Deserialize)]
pub struct Confirmation {
    pub prompt: Vec<Message>,
    pub settled_tool_call_ids: Vec<String>,
    pub response_id: Option<String>,
}
/// Disposable per-conversation optimization state, never a history authority.
pub trait Session: Send + Sync {
    fn stream(&self, request: Request, context: Context) -> BoxFuture<'static, Result<(), Error>>;
    fn needs_confirmation(&self) -> bool {
        false
    }
    fn confirm(&self, _confirmation: Confirmation) -> BoxFuture<'_, Result<bool, Error>> {
        Box::pin(async { Ok(false) })
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    Request,
    Conversation,
}
pub trait ProviderAdapter: Send + Sync {
    fn open(
        &self,
        lifetime: Lifetime,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<Arc<dyn Session>, Error>>;
}
pub struct Adapter {
    pub provider: Arc<dyn ProviderAdapter>,
}
/// A logical step captures the descriptor; retries never resolve the catalog.
#[derive(Clone)]
pub struct Binding(Contribution<Adapter>);
impl Binding {
    pub fn new(contribution: Contribution<Adapter>) -> Self {
        Self(contribution)
    }
    pub async fn open(
        &self,
        lifetime: Lifetime,
        cancellation: CancellationToken,
    ) -> Result<Arc<dyn Session>, Error> {
        let _call = self.admit()?;
        self.0.value.provider.open(lifetime, cancellation).await
    }
    pub fn same_registration(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0.value, &other.0.value)
    }
    pub fn source(&self, name: &str) -> Result<maka_runtime::composition::SourceRevision, Error> {
        let identity = self
            .0
            .owner
            .identity()
            .map_err(|error| Error::Adapter(error.to_string()))?;
        Ok(maka_runtime::composition::SourceRevision {
            kind: maka_runtime::composition::SourceKind::ModelAdapter,
            name: name.into(),
            package_id: identity.package_id,
            entry_id: identity.entry_id,
            revision: self.0.registration_id().to_string(),
            activation: identity.activation,
        })
    }
    pub fn admit(&self) -> Result<CallGuard, Error> {
        self.0
            .admit()
            .map_err(|_| Error::Adapter("model adapter registration retired".into()))
    }
}
