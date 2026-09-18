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

mod definitions;
mod operators;
mod read;
mod remote;
mod session;
mod tools;

use super::Setup;
use crate::execution::Executions;
use futures_util::future::BoxFuture;
use maka_event_log::EventLog;
use maka_graph::{Mode, owner::Handle};
use maka_plugins::{
    client::{Bundle, Client},
    composition::{Entry, Operation, Scope},
    contributions::{Catalog, Staged},
    fiber::Context,
    kernel::{Definition, Plugin, PluginContext},
    session::SessionBehavior,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub(crate) const ID: &str = "maka.agent-graph";

pub(crate) fn install(
    setup: &mut Setup,
    log: Arc<EventLog>,
    configuration: Arc<maka_config::ConfigurationStore>,
    executions: &Arc<Executions>,
    root: String,
) -> Result<(), maka_plugins::Error> {
    if setup.builtins.contains_key(ID) || setup.layers.contains_key(ID) {
        return Err(maka_plugins::Error::Invalid(
            "built-in Agent Graph identity is reserved".into(),
        ));
    }
    let bundle = Bundle::builtin(
        ID,
        env!("CARGO_PKG_VERSION"),
        include_str!(concat!(env!("OUT_DIR"), "/agent-graph-client.js")),
    )?;
    setup.builtins.insert(
        ID.into(),
        Arc::new(Definition {
            id: ID.into(),
            revision: env!("CARGO_PKG_VERSION").into(),
            dependencies: vec![],
            inject: vec![],
            plugin: Arc::new(GraphPlugin {
                bundle,
                log,
                configuration,
                executions: Arc::downgrade(executions),
                root,
                catalog: executions.plugin_catalog.clone(),
            }),
        }),
    );
    let mut entry = Entry::new(ID)?;
    entry.package_id = Some(ID.into());
    let mut client = Entry::new("maka.agent-graph.ui")?;
    client.package_id = Some(ID.into());
    setup.layers.insert(
        ID.into(),
        vec![
            Operation::Insert {
                root_id: Some(Scope::Profile),
                parent_id: None,
                position: None,
                entry,
            },
            Operation::Insert {
                root_id: Some(Scope::DesktopUi),
                parent_id: None,
                position: None,
                entry: client,
            },
        ],
    );
    Ok(())
}

struct GraphPlugin {
    bundle: Arc<Bundle>,
    log: Arc<EventLog>,
    configuration: Arc<maka_config::ConfigurationStore>,
    executions: Weak<Executions>,
    root: String,
    catalog: Catalog,
}
impl Plugin for GraphPlugin {
    fn supports_scope(&self, scope: &Scope) -> bool {
        matches!(scope, Scope::Profile | Scope::DesktopUi)
    }
    fn validate(&self, _: &Scope, config: &Value) -> Result<(), maka_plugins::Error> {
        if config.is_null() || config.as_object().is_some_and(|object| object.is_empty()) {
            Ok(())
        } else {
            Err(maka_plugins::Error::Invalid(
                "Agent Graph currently takes no instance configuration".into(),
            ))
        }
    }
    fn activate(
        &self,
        context: PluginContext,
        config: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        let bundle = self.bundle.clone();
        if context
            .lifecycle
            .identity()
            .is_ok_and(|identity| identity.scope == Scope::DesktopUi)
        {
            return Box::pin(async move {
                let identity = context.lifecycle.identity().map_err(error)?;
                let mut staged = Staged::default();
                staged
                    .insert(identity.entry_id, Client { bundle, config })
                    .map_err(error)?;
                Ok(staged)
            });
        }
        let manager = Arc::new(Manager {
            parent: context.lifecycle,
            log: self.log.clone(),
            configuration: self.configuration.clone(),
            executions: self.executions.clone(),
            root: self.root.clone(),
            catalog: self.catalog.clone(),
            roots: Mutex::default(),
            recovering: AtomicBool::new(true),
        });
        Box::pin(async move {
            let recovery = manager.clone();
            manager
                .parent
                .spawn("recover Agent Graph Sessions", async move {
                    let mut delay = Duration::from_millis(250);
                    loop {
                        if recovery.restore().await.is_ok() {
                            recovery.recovering.store(false, Ordering::Release);
                            return Ok(());
                        }
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(Duration::from_secs(30));
                    }
                })
                .map_err(error)?;
            let mut staged = Staged::default();
            remote::register(
                &mut staged,
                manager.log.clone(),
                manager.executions.clone(),
                manager.catalog.clone(),
                &bundle.content_digest,
            )?;
            staged
                .insert(
                    "agent-graph",
                    manager.clone() as Arc<dyn maka_plugins::background::BackgroundWork>,
                )
                .map_err(error)?;
            staged
                .insert("agent-graph", SessionBehavior(manager))
                .map_err(|error| error.to_string())?;
            Ok(staged)
        })
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Root>>>;
struct Manager {
    parent: Context,
    log: Arc<EventLog>,
    configuration: Arc<maka_config::ConfigurationStore>,
    executions: Weak<Executions>,
    root: String,
    catalog: Catalog,
    roots: Mutex<BTreeMap<String, Slot>>,
    recovering: AtomicBool,
}
impl maka_plugins::background::BackgroundWork for Manager {
    fn is_pending(&self) -> bool {
        if self.recovering.load(Ordering::Acquire) {
            return true;
        }
        self.roots.lock().unwrap().values().any(|slot| {
            let Ok(slot) = slot.try_lock() else {
                return true;
            };
            slot.as_ref().is_some_and(|root| {
                let view = root.handle.snapshot();
                !view.initialized || view.pending_work
            })
        })
    }
}
struct Root {
    lifecycle: Context,
    handle: Handle,
    mode: Mode,
}

/// Closing submission does not revoke reads or cancellation of already-owned work.
struct Submission {
    graph_id: maka_graph::GraphId,
    stop: tokio_util::sync::CancellationToken,
}

struct NativeOperators {
    commands: Arc<dyn maka_plugins::execution::Commands>,
    log: Arc<EventLog>,
    context: Context,
    root: String,
    graph_id: maka_graph::GraphId,
    definitions: definitions::Definitions,
    storage: Arc<dyn maka_plugins::storage::Store>,
}
fn error(error: impl ToString) -> String {
    error.to_string()
}
fn now() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(error)?
        .as_millis()
        .try_into()
        .map_err(|_| "system clock overflow".into())
}
