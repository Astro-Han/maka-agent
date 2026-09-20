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

use super::{ID, Service};
use crate::{
    authorization::Origin,
    command::{Mutation, Query},
};
use futures_util::future::BoxFuture;
use maka_plugins::{
    authorization::{Capability, Id, Request as Consent, Target},
    client::Bundle,
    contributions::Staged,
    remote::{Caller, Endpoint, Error, Handler, Method, Stream, StreamProvider, key},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Query {
        query: Query,
    },
    Mutate {
        mutation: Box<Mutation>,
        grant: Option<Id>,
    },
    Grants,
    RememberGrant {
        id: Id,
    },
    Session,
}
pub(super) fn publish(
    service: Service,
    bundle: &Bundle,
    staged: &mut Staged,
) -> Result<(), String> {
    staged
        .insert(
            key(ID, "request").map_err(super::display)?,
            Endpoint::new(
                bundle.content_digest.clone(),
                Handler::Method(Arc::new(service.clone())),
            ),
        )
        .map_err(super::display)?;
    staged
        .insert(
            key(ID, "changes").map_err(super::display)?,
            Endpoint::new(
                bundle.content_digest.clone(),
                Handler::Stream(Arc::new(ChangesProvider(service))),
            ),
        )
        .map_err(super::display)?;
    Ok(())
}
impl Method for Service {
    fn call(&self, input: Value, caller: Caller) -> BoxFuture<'static, Result<Value, Error>> {
        let service = self.clone();
        Box::pin(async move {
            let request: Request =
                serde_json::from_value(input).map_err(|error| Error::Invalid(error.to_string()))?;
            match request {
                Request::Query { query } => {
                    query.validate().map_err(error)?;
                    encode(service.query(query).map_err(error)?)
                }
                Request::Mutate { mutation, grant } => encode(
                    service
                        .mutate(*mutation, Origin::User { grant })
                        .await
                        .map_err(error)?,
                ),
                Request::Grants => encode(service.backend.grants().await.map_err(error)?),
                Request::RememberGrant { id } => {
                    encode(service.backend.remember_grant(id).await.map_err(error)?)
                }
                Request::Session => {
                    let session_id = caller
                        .session_id
                        .ok_or_else(|| Error::Invalid("Session context is required".into()))?;
                    let authorized = caller
                        .views
                        .authorize(Consent {
                            operation_id: uuid::Uuid::new_v4(),
                            title: "Inspect scheduling context".into(),
                            target: Target::Session {
                                session_id: session_id.clone(),
                            },
                            capabilities: [Capability::Executions].into(),
                        })
                        .await?;
                    let result = async {
                        let commands = service
                            .backend
                            .host
                            .executions
                            .acquire(authorized.scope())
                            .await
                            .map_err(|e| Error::Provider(e.to_string()))?;
                        commands
                            .session(session_id)
                            .await
                            .map_err(|e| Error::Provider(e.to_string()))
                    }
                    .await;
                    authorized
                        .finish()
                        .await
                        .map_err(|_| Error::CleanupUnconfirmed)?;
                    encode(result?)
                }
            }
        })
    }
}
fn encode(value: impl serde::Serialize) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(|error| Error::Invalid(error.to_string()))
}
fn error(error: crate::Error) -> Error {
    match error {
        crate::Error::OutcomeUnknown
        | crate::Error::Storage(maka_plugins::storage::StoreError::OutcomeUnknown(_)) => {
            Error::OutcomeUnknown(error.to_string())
        }
        crate::Error::Authority(maka_plugins::execution::CommandError::OutcomeUnknown(_))
        | crate::Error::Resource(maka_runtime::tools::ToolError::OutcomeUnknown(_)) => {
            Error::OutcomeUnknown(error.to_string())
        }
        crate::Error::Resource(
            maka_runtime::tools::ToolError::CleanupUnconfirmed(_)
            | maka_runtime::tools::ToolError::Persistence(_),
        ) => Error::CleanupUnconfirmed,
        crate::Error::Closed => Error::Retired,
        crate::Error::Invalid(_) | crate::Error::Time(_) => Error::Invalid(error.to_string()),
        _ => Error::Provider(error.to_string()),
    }
}
struct ChangesProvider(Service);
impl StreamProvider for ChangesProvider {
    fn open(
        &self,
        input: Value,
        caller: Caller,
    ) -> BoxFuture<'static, Result<Box<dyn Stream>, Error>> {
        let view = self.0.handle.subscribe();
        Box::pin(async move {
            if !input.is_null() {
                return Err(Error::Invalid("Changes takes no arguments".into()));
            }
            Ok(Box::new(Changes {
                state: Mutex::new((view, true)),
                stop: caller.cancellation.child_token(),
            }) as Box<dyn Stream>)
        })
    }
}
struct Changes {
    state: Mutex<(tokio::sync::watch::Receiver<Arc<crate::view::View>>, bool)>,
    stop: CancellationToken,
}
impl Stream for Changes {
    fn next(&self) -> BoxFuture<'_, Result<Option<Value>, Error>> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if self.stop.is_cancelled() {
                return Ok(None);
            }
            if state.1 {
                state.1 = false;
            } else {
                tokio::select! {
                    biased;
                    _ = self.stop.cancelled() => return Ok(None),
                    changed = state.0.changed() => changed.map_err(|_| Error::Retired)?,
                }
            }
            let view = state.0.borrow_and_update();
            Ok(Some(
                json!({"revision":view.revision,"ready":view.ready,"error":view.error}),
            ))
        })
    }
    fn cancel(&self) {
        self.stop.cancel();
    }
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), Error>> {
        self.stop.cancel();
        Box::pin(async { Ok(()) })
    }
}
