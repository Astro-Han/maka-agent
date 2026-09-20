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

//! Call ownership shared by native and JavaScript adapters, never a wire grant.

mod resources;
pub use resources::{Resources, Ticket};

use maka_runtime::{event::Invocation, tools::ToolError};
use serde::{Deserialize, Serialize};
use std::{any::Any, sync::Arc};
use tokio_util::sync::CancellationToken;

tokio::task_local! { static CURRENT: Scope; }

/// Available while polling an admitted plugin tool. Preparation and detached
/// tasks have no implicit authority; Service/Executor contexts pass it explicitly.
pub fn current() -> Option<Scope> {
    CURRENT.try_with(Clone::clone).ok()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Identity {
    Agent {
        invocation: Invocation,
        operation_id: Option<String>,
    },
    Remote {
        request_id: uuid::Uuid,
    },
    Background {
        grant: crate::authorization::Id,
    },
}
impl Identity {
    pub fn agent(&self) -> Option<&Invocation> {
        match self {
            Self::Agent { invocation, .. } => Some(invocation),
            _ => None,
        }
    }
    pub fn operation_id(&self) -> Option<&str> {
        match self {
            Self::Agent { operation_id, .. } => operation_id.as_deref(),
            _ => None,
        }
    }
}

/// Host dispatch creates the initial scope after authorization. Identity is
/// observation, not a grant: resource services must still validate admission.
/// Forwarding keeps revocation but owns a distinct resource settlement group.
#[derive(Clone)]
pub struct Scope {
    source: Arc<()>,
    // The embedding owns its authorization evidence. It is neither part of the
    // public observation nor serializable into a plugin-supplied wire grant.
    evidence: Option<Arc<dyn Any + Send + Sync>>,
    context: Metadata,
}

/// Read-only through Scope; observation cannot be rewritten into authority.
#[derive(Clone)]
pub struct Metadata {
    pub identity: Identity,
    pub cancellation: CancellationToken,
    pub resources: Arc<Resources>,
}

impl std::ops::Deref for Scope {
    type Target = Metadata;
    fn deref(&self) -> &Metadata {
        &self.context
    }
}

/// Embedding dispatch owns an issuer. A different embedding can create its own
/// issuer but cannot mint scopes accepted by this one's resource adapters.
#[derive(Clone, Default)]
pub struct Issuer(Arc<()>);
impl Issuer {
    pub async fn run<T>(
        &self,
        identity: Identity,
        cancellation: CancellationToken,
        operation: impl Future<Output = Result<T, ToolError>>,
    ) -> Result<T, ToolError> {
        let scope = self.issue(identity, cancellation)?;
        let _closed = scope.cancellation.clone().drop_guard();
        let result = CURRENT.scope(scope.clone(), operation).await;
        scope.finish().await?;
        result
    }
    pub fn issue(
        &self,
        identity: Identity,
        cancellation: CancellationToken,
    ) -> Result<Scope, ToolError> {
        Scope::new(self.0.clone(), identity, None, cancellation)
    }
    pub fn owns(&self, scope: &Scope) -> bool {
        Arc::ptr_eq(&self.0, &scope.source)
    }
    pub fn issue_with<T: Any + Send + Sync>(
        &self,
        identity: Identity,
        evidence: T,
        cancellation: CancellationToken,
    ) -> Result<Scope, ToolError> {
        Scope::new(
            self.0.clone(),
            identity,
            Some(Arc::new(evidence)),
            cancellation,
        )
    }
    /// Only the issuing embedding can inspect its private evidence type.
    pub fn evidence<'a, T: Any + Send + Sync>(&self, scope: &'a Scope) -> Option<&'a T> {
        self.owns(scope).then_some(())?;
        scope.evidence.as_ref()?.downcast_ref()
    }
}

impl Scope {
    fn new(
        source: Arc<()>,
        identity: Identity,
        evidence: Option<Arc<dyn Any + Send + Sync>>,
        cancellation: CancellationToken,
    ) -> Result<Self, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::Failed("plugin invocation is closed".into()));
        }
        Ok(Self {
            source,
            evidence,
            context: Metadata {
                identity,
                cancellation: cancellation.child_token(),
                resources: Arc::default(),
            },
        })
    }

    pub fn child(&self) -> Result<Self, ToolError> {
        Self::new(
            self.source.clone(),
            self.identity.clone(),
            self.evidence.clone(),
            self.cancellation.clone(),
        )
    }

    pub async fn finish(&self) -> Result<(), ToolError> {
        self.cancellation.cancel();
        self.resources.finish().await
    }
}

/// The caller owns this lifetime; forwarding a Scope does not transfer it.
/// Drop requests cancellation. `finish` additionally confirms resource cleanup.
pub struct Owned(Scope);
impl Owned {
    pub fn new(scope: Scope) -> Self {
        Self(scope)
    }
    pub fn scope(&self) -> Scope {
        self.0.clone()
    }
    pub async fn finish(self) -> Result<(), ToolError> {
        self.0.finish().await
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        self.0.cancellation.cancel();
    }
}
