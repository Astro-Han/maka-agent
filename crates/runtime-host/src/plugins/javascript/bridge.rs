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

mod service;
mod wire;

use futures_util::future::BoxFuture;
use maka_js_runtime::plugin::{Bridge, Error as VmError, Module, WeakModule};
use maka_plugins::{execution::Commands, kernel::PluginContext, services::Service, storage::Store};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use wire::{Error, Request};

struct JsService {
    callback: Arc<super::callbacks::Callback>,
}
#[derive(Clone)]
struct Reference {
    service: Service<JsService>,
    configuration: Vec<Value>,
}
struct State {
    source: Arc<super::remote::Source>,
    processes: super::process::Processes,
    http: super::http::Http,
    effects: super::effects::Effects,
    terminals: super::terminal::Terminals,
    calls: Arc<super::invocation::Calls>,
    outputs: Arc<super::executor::Outputs>,
    catalog: maka_plugins::contributions::Catalog,
    registrations: Mutex<BTreeMap<String, maka_plugins::contributions::Registration>>,
    context: PluginContext,
    storage: Arc<crate::plugins::storage::BoundStore>,
    commands: Arc<dyn Commands>,
    module: OnceLock<WeakModule>,
    handles: Mutex<BTreeMap<String, Reference>>,
}
pub(super) struct HostBridge(Arc<State>);
impl HostBridge {
    pub fn new(
        context: PluginContext,
        storage: Arc<crate::plugins::storage::BoundStore>,
        commands: Arc<dyn Commands>,
        catalog: maka_plugins::contributions::Catalog,
        executions: std::sync::Weak<crate::execution::Executions>,
        source: Arc<super::remote::Source>,
    ) -> Self {
        Self(Arc::new(State {
            source,
            http: super::http::Http::new(executions.clone(), context.lifecycle.clone()),
            effects: super::effects::Effects::new(executions.clone(), context.lifecycle.clone()),
            processes: super::process::Processes::new(
                executions.clone(),
                context.lifecycle.clone(),
            ),
            terminals: super::terminal::Terminals::new(
                executions.clone(),
                context.lifecycle.clone(),
            ),
            calls: Arc::default(),
            outputs: Arc::default(),
            catalog,
            registrations: Mutex::default(),
            context,
            storage,
            commands,
            module: OnceLock::new(),
            handles: Mutex::default(),
        }))
    }
    pub fn bind(&self, module: &Module) {
        self.0
            .module
            .set(module.downgrade())
            .ok()
            .expect("module is bound once");
    }
    pub fn outputs(&self) -> &Arc<super::executor::Outputs> {
        &self.0.outputs
    }
    pub fn calls(&self) -> &Arc<super::invocation::Calls> {
        &self.0.calls
    }
}
impl Bridge for HostBridge {
    fn max_input_bytes(&self, method: &str) -> usize {
        match method {
            // A byte can take four JSON bytes, plus bounded headers and URL.
            "http.request" => 5 * 1024 * 1024,
            "files.invoke" => 7 * 1024 * 1024,
            "llm.generate" => 2 * 1024 * 1024,
            _ => 1024 * 1024,
        }
    }
    fn call(&self, method: String, input: Value) -> BoxFuture<'static, Result<Value, VmError>> {
        let state = self.0.clone();
        Box::pin(async move {
            let request = serde_json::from_value(json!({ "method": method, "input": input }))
                .map_err(Error::invalid);
            let result = match request {
                Ok(request) => state.call(request).await,
                Err(error) => Err(error),
            };
            Ok(match result {
                Ok(value) => json!({"ok":true,"value":value}),
                Err(error) => json!({"ok":false,"error":error}),
            })
        })
    }
}
impl State {
    async fn call(&self, request: Request) -> Result<Value, Error> {
        // Initialization may access declared Services and data, not submit work.
        // Execution commands independently require effective business admission.
        let _lease = self.context.lifecycle.resource_call()?;
        match request {
            Request::CredentialRead(input) => encode(
                maka_plugins::credentials::Credentials::read(&*self.storage, input.key).await?,
            ),
            Request::CredentialWrite(input) => {
                encode(maka_plugins::credentials::Credentials::write(&*self.storage, input).await?)
            }
            Request::TerminalSpawn(input) => {
                let authority = self.calls.get(&input.authority)?;
                encode(
                    self.terminals
                        .spawn(authority, input)
                        .await
                        .map_err(Error::invalid)?,
                )
            }
            Request::TerminalControl(input) => {
                let authority = self.calls.get(&input.authority)?;
                encode(
                    self.terminals
                        .control(&authority, input)
                        .await
                        .map_err(Error::invalid)?,
                )
            }
            Request::TerminalNext(input) => {
                let authority = self.calls.get(&input.authority)?;
                encode(
                    self.terminals
                        .next(&authority, &input.handle)
                        .await
                        .map_err(Error::invalid)?,
                )
            }
            Request::TerminalWait(input) => {
                let authority = self.calls.get(&input.authority)?;
                encode(
                    self.terminals
                        .wait(&authority, &input.handle)
                        .await
                        .map_err(Error::invalid)?,
                )
            }
            Request::TerminalClose(input) => {
                self.terminals
                    .close(&input.handle)
                    .await
                    .map_err(Error::invalid)?;
                Ok(Value::Null)
            }
            Request::Files(input) => {
                let authority = self.calls.get(&input.authority)?;
                self.effects
                    .invoke(authority, super::effects::Effect::File(input.operation))
                    .await
                    .map_err(Error::tool)
            }
            Request::Generate(input) => {
                let authority = self.calls.get(&input.authority)?;
                self.effects
                    .invoke(authority, super::effects::Effect::Model(input.input))
                    .await
                    .map_err(Error::tool)
            }
            Request::ClientCatalog(input) => {
                let authority = self.calls.get(&input.authority)?;
                self.effects
                    .clients(authority)
                    .await
                    .map_err(Error::invalid)
            }
            Request::ClientCall(input) => {
                let authority = self.calls.get(&input.authority)?;
                self.effects
                    .invoke(authority, super::effects::Effect::Client(input.call))
                    .await
                    .map_err(Error::tool)
            }
            Request::HttpSend(input) => {
                let authority = self.calls.get(&input.authority)?;
                encode(
                    self.http
                        .request(authority, input)
                        .await
                        .map_err(Error::invalid)?,
                )
            }
            Request::HttpNext(input) => {
                let authority = self.calls.get(&input.authority)?;
                encode(
                    self.http
                        .next(&authority, &input.handle)
                        .await
                        .map_err(Error::invalid)?,
                )
            }
            Request::HttpClose(input) => {
                self.http
                    .close(&input.handle)
                    .await
                    .map_err(|message| Error {
                        code: wire::Code::OutcomeUnknown,
                        message,
                    })?;
                Ok(Value::Null)
            }
            Request::ProcessSpawn(input) => {
                let authority = self.calls.get(&input.authority)?;
                encode(
                    self.processes
                        .spawn(authority, input.command)
                        .await
                        .map_err(Error::invalid)?,
                )
            }
            Request::ProcessWrite(input) => {
                let authority = self.calls.get(&input.target.authority)?;
                self.processes
                    .write(&authority, &input.target.handle, input.bytes)
                    .await
                    .map_err(Error::invalid)?;
                Ok(Value::Null)
            }
            Request::ProcessEndInput(input) => {
                let authority = self.calls.get(&input.authority)?;
                self.processes
                    .end_input(&authority, &input.handle)
                    .await
                    .map_err(Error::invalid)?;
                Ok(Value::Null)
            }
            Request::ProcessNext(input) => {
                let authority = self.calls.get(&input.authority)?;
                encode(
                    self.processes
                        .next(&authority, &input.handle)
                        .await
                        .map_err(Error::invalid)?,
                )
            }
            Request::ProcessWait(input) => {
                let authority = self.calls.get(&input.authority)?;
                encode(
                    self.processes
                        .wait(&authority, &input.handle)
                        .await
                        .map_err(Error::invalid)?,
                )
            }
            Request::ProcessClose(input) => {
                self.processes
                    .close(&input.handle)
                    .await
                    .map_err(|message| Error {
                        code: wire::Code::OutcomeUnknown,
                        message,
                    })?;
                Ok(Value::Null)
            }
            Request::ExecutorEmit(input) => {
                self.outputs
                    .emit(&input.handle, input.output)
                    .await
                    .map_err(|error| Error {
                        code: wire::Code::Unavailable,
                        message: error.to_string(),
                    })?;
                Ok(Value::Null)
            }
            Request::Publish(registrations) => {
                let module = self
                    .module
                    .get()
                    .and_then(WeakModule::upgrade)
                    .ok_or(maka_plugins::Error::Retired)?;
                let staged = super::registration::stage_entries(
                    registrations,
                    &module,
                    &self.outputs,
                    &self.calls,
                    &self.source,
                )
                .map_err(Error::invalid)?;
                let mut registrations = self.registrations.lock().unwrap();
                if registrations.len() >= 128 {
                    return Err(Error::invalid("dynamic registration capacity exceeded"));
                }
                let registration = self.catalog.register(&self.context.lifecycle, staged)?;
                let id = uuid::Uuid::new_v4().to_string();
                registrations.insert(id.clone(), registration);
                Ok(json!(id))
            }
            Request::Unpublish(input) => {
                self.registrations.lock().unwrap().remove(&input.handle);
                Ok(Value::Null)
            }
            Request::Withdraw(input) => {
                super::registration::withdraw(
                    &self.catalog,
                    &self.context.lifecycle,
                    input.kind,
                    &input.name,
                )?;
                Ok(Value::Null)
            }
            Request::Read(input) => encode(self.storage.read(input.key).await?),
            Request::Batch(input) => encode(self.storage.batch(input.mutations).await?),
            Request::Submit(input) => encode(self.commands.submit(input).await?),
            Request::CreateChild(input) => encode(self.commands.create_child(input).await?),
            Request::WorkspacePatch(input) => {
                encode(self.commands.workspace_patch(input.operation_id).await?)
            }
            Request::Query(input) => encode(self.commands.query(input.operation_id).await?),
            Request::Cancel(input) => encode(self.commands.cancel(input.operation_id).await?),
            Request::Events(input) => encode(
                self.commands
                    .events(input.operation_id, input.after, input.through)
                    .await?,
            ),
            Request::Event(input) => encode(
                self.commands
                    .event(input.operation_id, input.event_id, input.through)
                    .await?,
            ),
            Request::Provide(input) => {
                if input.callback == 0 {
                    return Err(Error::invalid("invalid service callback"));
                }
                let module = self
                    .module
                    .get()
                    .and_then(WeakModule::upgrade)
                    .ok_or(maka_plugins::Error::Retired)?;
                let callback = Arc::new(super::callbacks::Callback {
                    module,
                    id: input.callback,
                    calls: self.calls.clone(),
                });
                let mut registrations = self.registrations.lock().unwrap();
                if registrations.len() >= 128 {
                    return Err(Error::invalid("dynamic registration capacity exceeded"));
                }
                let registration = self.context.services.register(
                    &self.context.lifecycle,
                    &input.name,
                    Arc::new(JsService { callback }),
                )?;
                let id = uuid::Uuid::new_v4().to_string();
                registrations.insert(id.clone(), registration);
                Ok(json!(id))
            }
            Request::Get(input) => {
                let Some(service) = self.context.services.get::<JsService>(&input.name)? else {
                    return Ok(Value::Null);
                };
                let mut handles = self.handles.lock().unwrap();
                if handles.len() >= 128 {
                    return Err(Error::invalid("service handle limit exceeded"));
                }
                let id = uuid::Uuid::new_v4().to_string();
                handles.insert(
                    id.clone(),
                    Reference {
                        service,
                        configuration: self.context.services.intercepts(&input.name).to_vec(),
                    },
                );
                Ok(json!(id))
            }
            Request::Call(input) => self.call_service(input).await,
            Request::Release(input) => {
                self.handles.lock().unwrap().remove(&input.handle);
                Ok(Value::Null)
            }
            Request::Sleep(input) => {
                if input.milliseconds > 86_400_000 {
                    return Err(Error::invalid("sleep exceeds one day"));
                }
                let stopping = self.context.lifecycle.stopping()?;
                tokio::select! {
                    biased;
                    _ = stopping.cancelled() => Err(maka_plugins::Error::Retired.into()),
                    _ = tokio::time::sleep(Duration::from_millis(input.milliseconds)) => Ok(Value::Null),
                }
            }
        }
    }
}
fn encode(value: impl Serialize) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(Error::invalid)
}
