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

use super::read::{self, Query};
use futures_util::future::BoxFuture;
use maka_event_log::EventLog;
use maka_graph::{
    GraphId,
    owner::{Handle, View},
};
use maka_plugins::{
    composition::Scope,
    contributions::{Catalog, Staged},
    remote::{Caller, Endpoint, Error, Handler, Method, Stream, StreamProvider, key},
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore, watch};
use tokio_util::sync::CancellationToken;

pub(super) fn register(
    staged: &mut Staged,
    log: Arc<EventLog>,
    sessions: Arc<dyn super::host::Sessions>,
    catalog: Catalog,
    digest: &str,
) -> Result<(), String> {
    let service = Arc::new(Service {
        log,
        sessions,
        catalog,
        reads: Semaphore::new(2),
    });
    for (name, handler) in [
        (
            "query",
            Handler::Method(Arc::new(Call {
                service: service.clone(),
                action: Action::Query,
            }) as Arc<dyn Method>),
        ),
        (
            "stop",
            Handler::Method(Arc::new(Call {
                service: service.clone(),
                action: Action::Stop,
            }) as Arc<dyn Method>),
        ),
        (
            "changes",
            Handler::Stream(service as Arc<dyn StreamProvider>),
        ),
    ] {
        staged
            .insert(
                key(super::ID, name).map_err(message)?,
                Endpoint::new(digest.into(), handler),
            )
            .map_err(message)?;
    }
    Ok(())
}
struct Service {
    log: Arc<EventLog>,
    sessions: Arc<dyn super::host::Sessions>,
    catalog: Catalog,
    reads: Semaphore,
}
struct Call {
    service: Arc<Service>,
    action: Action,
}
#[derive(Clone, Copy)]
enum Action {
    Query,
    Stop,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Stop {
    graph_id: GraphId,
}
impl Method for Call {
    fn call(&self, input: Value, caller: Caller) -> BoxFuture<'static, Result<Value, Error>> {
        let service = self.service.clone();
        let action = self.action;
        Box::pin(async move {
            let root = caller
                .session_id
                .ok_or_else(|| Error::Invalid("Agent Graph requires a Session".into()))?;
            match action {
                Action::Query => {
                    let query: Query = serde_json::from_value(input)
                        .map_err(|error| Error::Invalid(error.to_string()))?;
                    let read = async {
                        let _permit = service.reads.acquire().await.map_err(|_| Error::Retired)?;
                        let value = read::query(&service.log, &root, query).await?;
                        serde_json::to_value(value).map_err(failure)
                    };
                    tokio::select! {
                        biased;
                        _ = caller.cancellation.cancelled() => Err(Error::Cancelled),
                        result = read => result,
                    }
                }
                Action::Stop => {
                    let input: Stop = serde_json::from_value(input)
                        .map_err(|error| Error::Invalid(error.to_string()))?;
                    if caller.cancellation.is_cancelled() {
                        return Err(Error::Cancelled);
                    }
                    service
                        .sessions
                        .stop(root, input.graph_id, caller.cancellation)
                        .await?;
                    Ok(Value::Null)
                }
            }
        })
    }
}
impl StreamProvider for Service {
    fn open(
        &self,
        input: Value,
        caller: Caller,
    ) -> BoxFuture<'static, Result<Box<dyn Stream>, Error>> {
        let catalog = self.catalog.clone();
        Box::pin(async move {
            if !input.is_null() || caller.session_id.is_none() {
                return Err(Error::Invalid(
                    "Graph changes require a Session and null input".into(),
                ));
            }
            Ok(Box::new(Changes {
                state: Mutex::new(Watch {
                    catalog: catalog.subscribe(),
                    view: None,
                    previous: None,
                    initialized: false,
                }),
                catalog,
                scope: Scope::Session(caller.session_id.expect("validated Session")),
                stop: caller.cancellation,
            }) as Box<dyn Stream>)
        })
    }
}
struct Changes {
    catalog: Catalog,
    scope: Scope,
    state: Mutex<Watch>,
    stop: CancellationToken,
}
struct Watch {
    catalog: watch::Receiver<u64>,
    view: Option<watch::Receiver<Arc<View>>>,
    previous: Option<Value>,
    initialized: bool,
}
impl Stream for Changes {
    fn next(&self) -> BoxFuture<'_, Result<Option<Value>, Error>> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            loop {
                if self.stop.is_cancelled() {
                    return Ok(None);
                }
                if !state.initialized || state.catalog.has_changed().map_err(|_| Error::Retired)? {
                    state.catalog.borrow_and_update();
                    state.view = self
                        .catalog
                        .snapshot::<Handle>(&self.scope)
                        .entries
                        .get("agent-graph")
                        .map(|handle| handle.value.subscribe());
                    state.initialized = true;
                }
                let key = state.view.as_mut().map_or(Value::Null, |changes| {
                    let view = changes.borrow_and_update();
                    serde_json::json!([view.change_key, view.error, view.initialized])
                });
                if state.previous.as_ref() != Some(&key) {
                    state.previous = Some(key);
                    return Ok(Some(Value::Null));
                }
                let Watch { catalog, view, .. } = &mut *state;
                tokio::select! {
                    biased;
                    _ = self.stop.cancelled() => return Ok(None),
                    result = catalog.changed() => {
                        result.map_err(|_| Error::Retired)?;
                        state.initialized = false;
                    },
                    result = async {
                        match view {
                            Some(view) => view.changed().await,
                            None => std::future::pending().await,
                        }
                    } => result.map_err(|_| Error::Retired)?,
                }
            }
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
fn message(error: impl ToString) -> String {
    error.to_string()
}
fn failure(error: impl ToString) -> Error {
    Error::Provider(error.to_string())
}
