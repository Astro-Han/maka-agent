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

use super::callbacks::{Callback, invoke};
use futures_util::future::BoxFuture;
use maka_plugins::{http, model::*};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

#[derive(Default)]
pub(super) struct Calls(Mutex<BTreeMap<String, Weak<Call>>>);
struct Call {
    context: Context,
    resources: Arc<Resources>,
    bodies: Mutex<BTreeMap<String, Arc<dyn http::Body>>>,
}
#[derive(Default)]
struct Resources {
    sockets: Mutex<BTreeMap<String, Arc<dyn Socket>>>,
}
struct Handle {
    calls: Arc<Calls>,
    id: String,
    call: Arc<Call>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.calls.0.lock().unwrap().remove(&self.id);
        for body in self.call.bodies.lock().unwrap().values() {
            body.cancel();
        }
    }
}
impl Calls {
    fn register(
        self: &Arc<Self>,
        context: Context,
        resources: Arc<Resources>,
    ) -> Result<Handle, Error> {
        let mut calls = self.0.lock().unwrap();
        if calls.len() >= 128 {
            return Err(invalid("model adapter call capacity exceeded"));
        }
        let call = Arc::new(Call {
            context,
            resources,
            bodies: Mutex::default(),
        });
        let id = uuid::Uuid::new_v4().to_string();
        calls.insert(id.clone(), Arc::downgrade(&call));
        Ok(Handle {
            calls: self.clone(),
            id,
            call,
        })
    }
    pub async fn execute(&self, request: Operation) -> Result<Value, Error> {
        let call = self
            .0
            .lock()
            .unwrap()
            .get(&request.handle)
            .and_then(Weak::upgrade)
            .ok_or_else(|| invalid("model call has settled"))?;
        let result = tokio::select! {
            biased;
            _ = call.context.cancellation.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(call.context.idle_timeout, call.execute(request.operation)) =>
                result.map_err(|_| Error::TimedOut)?,
        };
        if result.is_ok() {
            call.context.events.progress();
        }
        result
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Operation {
    handle: String,
    #[serde(flatten)]
    operation: Io,
}
#[derive(Deserialize)]
#[serde(tag = "kind", content = "input", rename_all = "snake_case")]
enum Io {
    Progress,
    Emit(ModelEvent),
    Request(http::Request),
    Read(String),
    CloseBody(String),
    Connect(Connect),
    Send { socket: String, frame: Frame },
    Receive(String),
    CloseSocket(String),
}
impl Call {
    fn socket(&self, id: &str) -> Result<Arc<dyn Socket>, Error> {
        self.resources
            .sockets
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| invalid("socket is closed"))
    }
    async fn execute(&self, operation: Io) -> Result<Value, Error> {
        match operation {
            Io::Progress => {
                self.context.events.progress();
                Ok(Value::Null)
            }
            Io::Emit(event) => {
                self.context.events.emit(event).await?;
                Ok(Value::Null)
            }
            Io::Request(request) => {
                let response = self.context.transport.request(request).await?;
                let mut bodies = self.bodies.lock().unwrap();
                if bodies.len() >= 16 {
                    return Err(invalid("too many open response bodies"));
                }
                let id = uuid::Uuid::new_v4().to_string();
                let value = json!({"id":id, "head":response.head});
                bodies.insert(id, response.body);
                Ok(value)
            }
            Io::Read(id) => {
                let body = self
                    .bodies
                    .lock()
                    .unwrap()
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| invalid("body is closed"))?;
                let result = body.next().await.map_err(|error| match error {
                    http::Error::Failed(_) => {
                        Error::Provider(maka_runtime::model::error::ProviderFailure::new(
                            maka_runtime::model::error::ProviderFailureReason::Network,
                            "model HTTP stream interrupted",
                            false,
                            None,
                        ))
                    }
                    http::Error::Denied => Error::Cancelled,
                    other => invalid(other.to_string()),
                })?;
                if result.is_none() {
                    self.bodies.lock().unwrap().remove(&id);
                }
                encode(result)
            }
            Io::CloseBody(id) => {
                let body = self.bodies.lock().unwrap().remove(&id);
                if let Some(body) = body {
                    body.close()
                        .await
                        .map_err(|error| invalid(error.to_string()))?;
                }
                Ok(Value::Null)
            }
            Io::Connect(request) => {
                let socket = self.context.transport.connect(request).await?;
                let mut sockets = self.resources.sockets.lock().unwrap();
                if sockets.len() >= 4 {
                    return Err(invalid("too many model sockets"));
                }
                let id = uuid::Uuid::new_v4().to_string();
                sockets.insert(id.clone(), socket);
                Ok(json!(id))
            }
            Io::Send { socket, frame } => {
                self.socket(&socket)?.send(frame).await?;
                Ok(Value::Null)
            }
            Io::Receive(id) => encode(self.socket(&id)?.receive().await?),
            Io::CloseSocket(id) => {
                let socket = self.resources.sockets.lock().unwrap().remove(&id);
                if let Some(socket) = socket {
                    socket.close().await?;
                }
                Ok(Value::Null)
            }
        }
    }
}
pub(super) struct Adapter {
    pub callback: Arc<Callback>,
    pub calls: Arc<Calls>,
}
impl ProviderAdapter for Adapter {
    fn open(
        &self,
        lifetime: Lifetime,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> BoxFuture<'_, Result<Arc<dyn Session>, Error>> {
        Box::pin(async move {
            let value = invoke(
                &self.callback.module,
                self.callback.id,
                json!(lifetime),
                Value::Null,
                cancellation,
            )
            .await
            .map_err(|error| invalid(error.to_string()))?;
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Opened {
                callback: u32,
                confirmation: bool,
            }
            let opened: Opened = serde_json::from_value(value).map_err(invalid)?;
            if opened.callback == 0 {
                return Err(invalid("invalid model session callback"));
            }
            Ok(Arc::new(JavaScript {
                callback: Arc::new(Callback {
                    module: self.callback.module.clone(),
                    id: opened.callback,
                    calls: self.callback.calls.clone(),
                }),
                calls: self.calls.clone(),
                resources: Arc::default(),
                confirmation: Mutex::default(),
                conversation: matches!(lifetime, Lifetime::Conversation) && opened.confirmation,
            }) as Arc<dyn Session>)
        })
    }
}
struct JavaScript {
    callback: Arc<Callback>,
    calls: Arc<Calls>,
    resources: Arc<Resources>,
    confirmation: Mutex<Option<Confirmation>>,
    conversation: bool,
}
impl Session for JavaScript {
    fn stream(&self, request: Request, context: Context) -> BoxFuture<'static, Result<(), Error>> {
        let callback = self.callback.clone();
        let calls = self.calls.clone();
        let resources = self.resources.clone();
        let confirmation = self.confirmation.lock().unwrap().take();
        Box::pin(async move {
            let cancellation = context.cancellation.clone();
            let routing = context.transport.identity().to_string();
            let handle = calls.register(context, resources)?;
            let result = invoke(
                &callback.module,
                callback.id,
                encode(request)?,
                json!({"model":handle.id, "routing":routing, "confirmation":confirmation}),
                cancellation.clone(),
            )
            .await
            .map_err(|error| invalid(error.to_string()))?;
            if cancellation.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if let Some(error) = result.get("error") {
                let error: Error = serde_json::from_value(error.clone()).map_err(invalid)?;
                if let Error::Provider(failure) = &error {
                    failure.validate()?;
                }
                return Err(error);
            }
            Ok(())
        })
    }
    fn needs_confirmation(&self) -> bool {
        self.conversation
    }
    fn confirm(&self, confirmation: Confirmation) -> BoxFuture<'_, Result<bool, Error>> {
        *self.confirmation.lock().unwrap() = Some(confirmation);
        Box::pin(async { Ok(true) })
    }
}
fn invalid(message: impl ToString) -> Error {
    Error::Adapter(message.to_string())
}
fn encode(value: impl Serialize) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(invalid)
}
