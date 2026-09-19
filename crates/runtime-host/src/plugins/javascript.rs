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

mod bridge;
mod callbacks;
mod effects;
mod executor;
mod http;
mod input;
mod invocation;
mod process;
mod registration;
mod remote;
mod terminal;

use super::PackageLoader;
use crate::execution::Executions;
use futures_util::future::BoxFuture;
use maka_js_runtime::plugin::{Lifecycle, Limits, Module, Pool};
use maka_plugins::{
    composition::Scope,
    contributions::Staged,
    fiber::Effect,
    kernel::{Definition, Plugin, PluginContext},
    package::{Package, VmMode},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

pub(crate) struct Loader {
    pool: Arc<Pool>,
    executions: Weak<Executions>,
    root: String,
}
impl Loader {
    pub fn new(executions: &Arc<Executions>, root: String) -> Result<Self, maka_plugins::Error> {
        Ok(Self {
            pool: Arc::new(Pool::new(Limits::default(), 4).map_err(invalid)?),
            executions: Arc::downgrade(executions),
            root,
        })
    }
}
impl PackageLoader for Loader {
    fn definition(&self, package: &Package) -> Result<Arc<Definition>, maka_plugins::Error> {
        let host = package.manifest().runtime.as_ref().map(|entry| {
            let host = (|| {
                package.manifest().require_host_sdk()?;
                let source = std::str::from_utf8(
                    package
                        .file(&entry.entry)
                        .ok_or_else(|| invalid("Host entrypoint bytes are missing"))?,
                )
                .map_err(invalid)?;
                Ok::<Arc<dyn Plugin>, maka_plugins::Error>(Arc::new(JavaScript {
                    pool: self.pool.clone(),
                    executions: self.executions.clone(),
                    root: self.root.clone(),
                    source: source.into(),
                    name: entry.entry.clone(),
                    mode: entry.vm,
                    generation: format!("{}:{}", package.manifest().id, package.digest()),
                    remote: Arc::new(remote::Source {
                        package_id: package.manifest().id.clone(),
                        content_digest: package.digest().into(),
                    }),
                }))
            })();
            host.unwrap_or_else(|error| Arc::new(super::authority::Unavailable(error.to_string())))
        });
        Ok(Arc::new(Definition {
            id: package.manifest().id.clone(),
            revision: package.digest().into(),
            dependencies: package
                .manifest()
                .dependencies
                .iter()
                .map(|item| item.id.clone())
                .collect(),
            inject: vec![],
            plugin: Arc::new(super::entrypoint::Entrypoint {
                host,
                client: maka_plugins::client::Bundle::from_package(package)?,
            }),
        }))
    }
}
struct JavaScript {
    remote: Arc<remote::Source>,
    pool: Arc<Pool>,
    executions: Weak<Executions>,
    root: String,
    source: String,
    name: String,
    generation: String,
    mode: VmMode,
}
impl Plugin for JavaScript {
    fn activate(
        &self,
        context: PluginContext,
        config: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        let pool = self.pool.clone();
        let executions = self.executions.clone();
        let remote = self.remote.clone();
        let (root, source, name, generation, mode) = (
            self.root.clone(),
            self.source.clone(),
            self.name.clone(),
            self.generation.clone(),
            self.mode,
        );
        Box::pin(async move {
            let executions = executions.upgrade().ok_or("Host closed")?;
            let identity = context.lifecycle.identity().map_err(message)?;
            let sessions = match &identity.scope {
                Scope::Session(id) => vec![id.clone()],
                _ => vec![],
            };
            let commands = executions
                .authorize_plugin(
                    context.lifecycle.clone(),
                    &sessions,
                    &root,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
                .map_err(message)?;
            let storage = executions
                .plugin_store(context.lifecycle.clone())
                .map_err(message)?;
            let vm = match mode {
                VmMode::Shared => pool.shared(),
                VmMode::Dedicated => pool.dedicated(&generation),
            }
            .map_err(message)?;
            let bridge = Arc::new(bridge::HostBridge::new(
                context.clone(),
                storage,
                commands,
                executions.plugin_catalog.clone(),
                Arc::downgrade(&executions),
                remote.clone(),
            ));
            let module = vm
                .load_plugin(name, source, bridge.clone())
                .map_err(message)?;
            bridge.bind(&module);
            own(&context, &module)?;
            let registrations = module
                .call(vec!["activate".into()], vec![json!(identity), config])
                .await
                .map_err(message)?;
            let staged = registration::stage(
                registrations,
                &module,
                bridge.outputs(),
                bridge.calls(),
                &remote,
            )?;
            let business = module.clone();
            let owner = context.lifecycle.clone();
            context
                .lifecycle
                .spawn("JavaScript business tasks", async move {
                    if let Err(error) = business.call(vec!["effective".into()], vec![]).await {
                        owner.retire();
                        return Err(error.to_string());
                    }
                    Ok(())
                })
                .map_err(message)?;
            let owner = context.lifecycle.clone();
            context
                .lifecycle
                .spawn("JavaScript VM health", async move {
                    let error = vm.failed().await;
                    owner.retire();
                    Err(error.to_string())
                })
                .map_err(message)?;
            Ok(staged)
        })
    }
}

fn own(context: &PluginContext, module: &Module) -> Result<(), String> {
    let retiring = module.clone();
    let closing = module.clone();
    let signal = Arc::new(Mutex::new(None));
    let signalled = signal.clone();
    let effect = Effect::new(
        "JavaScript instance",
        move || {
            let task = tokio::spawn(async move {
                // This also bounds retirement of a hung asynchronous JS callback.
                let stopped = tokio::time::timeout(Duration::from_secs(5), async {
                    match retiring.ready().await {
                        Ok(()) => retiring.lifecycle(Lifecycle::Retire).await.map(|()| true),
                        Err(_) if !retiring.vm_failed() => Ok(false),
                        Err(error) => Err(error),
                    }
                })
                .await;
                let result = match stopped {
                    Ok(result) => result.map_err(message),
                    Err(_) => Err("plugin retirement did not acknowledge".into()),
                };
                if result.is_err() {
                    retiring.terminate_vm("plugin retirement did not acknowledge");
                }
                result
            });
            *signal.lock().unwrap() = Some(task);
        },
        move || async move {
            let task = signalled.lock().unwrap().take();
            let stopped = match task {
                Some(task) => task.await.map_err(message)?,
                None => Err("plugin retirement task disappeared".into()),
            };
            if !matches!(stopped, Ok(true)) {
                closing.close().await.map_err(message)?;
                return stopped.map(|_| ());
            }
            let disposed = tokio::time::timeout(
                Duration::from_secs(5),
                closing.lifecycle(Lifecycle::Dispose),
            )
            .await;
            let result = match disposed {
                Ok(result) => result.map_err(message),
                Err(_) => {
                    closing.terminate_vm("plugin cleanup deadline exceeded");
                    Err("JavaScript cleanup did not complete".into())
                }
            };
            closing.close().await.map_err(message)?;
            result
        },
    );
    context.lifecycle.own(effect).map_err(|effect| {
        drop(effect);
        "plugin retired during module loading".into()
    })
}

fn message(error: impl ToString) -> String {
    error.to_string()
}
fn invalid(error: impl ToString) -> maka_plugins::Error {
    maka_plugins::Error::Invalid(error.to_string())
}
