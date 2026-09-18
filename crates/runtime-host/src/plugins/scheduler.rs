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

mod authorization;
mod changes;
mod delivery;
mod notification;
mod tools;

use super::Setup;
use crate::{execution::Executions, server::capabilities::Capabilities};
use futures_util::future::BoxFuture;
use maka_plugins::{
    composition::{Entry, Operation, Scope},
    contributions::Staged,
    fiber::Context,
    kernel::{Definition, Plugin, PluginContext},
};
use maka_scheduler::{
    authorization::Origin,
    command::{Mutation, MutationResult, Query, QueryResult},
    controller::Controller,
    delivery::SystemClock,
    owner::Handle,
    plan::Misfire,
    repository::Repository,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::{Arc, Weak};

pub(crate) const ID: &str = "maka.scheduler";

pub(crate) fn install(
    setup: &mut Setup,
    log: Arc<maka_event_log::EventLog>,
    configuration: Arc<maka_config::ConfigurationStore>,
    executions: &Arc<Executions>,
    capabilities: Arc<Capabilities>,
    root_id: String,
    changes: tokio::sync::broadcast::Sender<Value>,
) -> Result<(), maka_plugins::Error> {
    if setup.builtins.contains_key(ID) || setup.layers.contains_key(ID) {
        return Err(maka_plugins::Error::Invalid(
            "built-in Scheduler identity is reserved".into(),
        ));
    }
    setup.builtins.insert(
        ID.into(),
        Arc::new(Definition {
            id: ID.into(),
            revision: env!("CARGO_PKG_VERSION").into(),
            dependencies: vec![],
            inject: vec![],
            plugin: Arc::new(Scheduler {
                log,
                configuration,
                executions: Arc::downgrade(executions),
                capabilities,
                root_id,
                changes,
            }),
        }),
    );
    let mut entry = Entry::new(ID)?;
    entry.package_id = Some(ID.into());
    setup.layers.insert(
        ID.into(),
        vec![Operation::Insert {
            root_id: Some(Scope::Profile),
            parent_id: None,
            position: None,
            entry,
        }],
    );
    Ok(())
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    timezone: Option<String>,
    #[serde(default)]
    misfire: Misfire,
}
impl Config {
    fn parse(value: Value) -> Result<Self, String> {
        if value.is_null() {
            Ok(Self::default())
        } else {
            serde_json::from_value(value).map_err(|error| error.to_string())
        }
    }
    fn timezone(&self) -> Result<String, String> {
        if let Some(zone) = &self.timezone {
            jiff::tz::TimeZone::get(zone).map_err(|error| error.to_string())?;
            return Ok(zone.clone());
        }
        let zone = jiff::tz::TimeZone::try_system().map_err(|error| error.to_string())?;
        zone.iana_name()
            .map(str::to_owned)
            .ok_or_else(|| "Configure an IANA timezone for the Scheduler".into())
    }
}
struct Scheduler {
    log: Arc<maka_event_log::EventLog>,
    configuration: Arc<maka_config::ConfigurationStore>,
    executions: Weak<Executions>,
    capabilities: Arc<Capabilities>,
    root_id: String,
    changes: tokio::sync::broadcast::Sender<Value>,
}
impl Plugin for Scheduler {
    fn supports_scope(&self, scope: &Scope) -> bool {
        matches!(scope, Scope::Profile)
    }
    fn validate(&self, _: &Scope, config: &Value) -> Result<(), maka_plugins::Error> {
        Config::parse(config.clone())
            .and_then(|config| config.timezone())
            .map(|_| ())
            .map_err(maka_plugins::Error::Invalid)
    }
    fn activate(
        &self,
        context: PluginContext,
        config: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        let changes = self.changes.clone();
        let backend = Arc::new(Backend {
            context: context.lifecycle.clone(),
            log: self.log.clone(),
            configuration: self.configuration.clone(),
            executions: self.executions.clone(),
            capabilities: self.capabilities.clone(),
            root_id: self.root_id.clone(),
        });
        Box::pin(async move {
            let config = Config::parse(config)?;
            let identity = context.lifecycle.identity().map_err(display)?;
            let host = backend.host().map_err(display)?;
            let repository = Repository::new(
                host.plugin_store(context.lifecycle.clone())
                    .map_err(display)?,
                &identity.entry_id,
            )
            .map_err(display)?;
            let controller = Controller::open(
                repository,
                config.timezone()?,
                jiff::Timestamp::now().as_millisecond(),
            )
            .await
            .map_err(display)?
            .with_misfire(config.misfire);
            let (handle, owner) = maka_scheduler::owner::start(
                controller,
                backend.clone(),
                Arc::new(SystemClock),
                context.lifecycle.stopping().map_err(display)?,
            );
            context
                .lifecycle
                .spawn("scheduled tasks", owner)
                .map_err(display)?;
            context
                .lifecycle
                .spawn(
                    "scheduled task changes",
                    changes::publish(handle.subscribe(), changes),
                )
                .map_err(display)?;
            let mut staged = Staged::default();
            staged
                .insert(
                    identity.entry_id.clone(),
                    Arc::new(handle.clone()) as Arc<dyn maka_plugins::background::BackgroundWork>,
                )
                .map_err(display)?;
            let service = Arc::new(Service {
                context: context.lifecycle,
                handle,
            });
            staged
                .insert("ScheduledTask", tools::register(service.clone(), backend)?)
                .map_err(display)?;
            staged
                .insert(identity.entry_id, (*service).clone())
                .map_err(display)?;
            Ok(staged)
        })
    }
}
#[derive(Clone)]
pub(crate) struct Service {
    context: Context,
    pub(crate) handle: Handle,
}
impl Service {
    pub(crate) fn query(&self, query: Query) -> Result<QueryResult, maka_scheduler::Error> {
        let _lease = self
            .context
            .admit()
            .map_err(|_| maka_scheduler::Error::Closed)?;
        self.handle.query(query)
    }
    pub(crate) async fn mutate(
        &self,
        mutation: Mutation,
        origin: Origin,
    ) -> Result<MutationResult, maka_scheduler::Error> {
        let _lease = self
            .context
            .admit()
            .map_err(|_| maka_scheduler::Error::Closed)?;
        self.handle.mutate(mutation, origin).await
    }
}
struct Backend {
    context: Context,
    log: Arc<maka_event_log::EventLog>,
    configuration: Arc<maka_config::ConfigurationStore>,
    executions: Weak<Executions>,
    capabilities: Arc<Capabilities>,
    root_id: String,
}
impl Backend {
    fn host(&self) -> Result<Arc<Executions>, maka_plugins::execution::CommandError> {
        self.executions
            .upgrade()
            .filter(|host| host.accepting())
            .ok_or(maka_plugins::execution::CommandError::Draining)
    }
    async fn privacy_allows(&self) -> Result<bool, maka_scheduler::Error> {
        self.configuration
            .runtime_policy()
            .await
            .map(|policy| !policy.policy.privacy.incognito_active)
            .map_err(|error| maka_scheduler::Error::Unavailable(error.to_string()))
    }
}
fn display(error: impl ToString) -> String {
    error.to_string()
}
