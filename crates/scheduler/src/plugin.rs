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

mod backend;
mod remote;
mod tools;

use crate::{
    authorization::Origin,
    command::{Mutation, MutationResult, Query, QueryResult},
    controller::Controller,
    delivery::SystemClock,
    owner::Handle,
    plan::Misfire,
    repository::Repository,
};
use futures_util::future::BoxFuture;
use maka_plugins::{
    client::{Bundle, Client},
    composition::Scope,
    contributions::Staged,
    fiber::Context,
    kernel::{Plugin, PluginContext},
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub const ID: &str = "maka.scheduler";
pub const CLIENT_SERVICE: &str = "maka.scheduler.client";
pub struct Builtin {
    pub client: Arc<Bundle>,
}
struct ClientSupport(Arc<Bundle>);
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
            serde_json::from_value(value).map_err(display)
        }
    }
    fn timezone(&self) -> Result<String, String> {
        if let Some(zone) = &self.timezone {
            jiff::tz::TimeZone::get(zone).map_err(display)?;
            return Ok(zone.clone());
        }
        jiff::tz::TimeZone::try_system()
            .map_err(display)?
            .iana_name()
            .map(str::to_owned)
            .ok_or_else(|| "Configure an IANA timezone for Scheduler".into())
    }
}
impl Plugin for Builtin {
    fn supports_scope(&self, scope: &Scope) -> bool {
        matches!(scope, Scope::Profile | Scope::DesktopUi)
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
        let bundle = self.client.clone();
        Box::pin(async move {
            let identity = context.lifecycle.identity().map_err(display)?;
            let mut staged = Staged::default();
            if identity.scope == Scope::DesktopUi {
                let provider = context
                    .services
                    .get::<ClientSupport>(CLIENT_SERVICE)
                    .map_err(display)?
                    .ok_or("Scheduler backend is unavailable")?;
                let client = provider.acquire().map_err(display)?;
                staged
                    .insert(
                        identity.entry_id,
                        Client {
                            bundle: client.0.clone(),
                            config,
                        },
                    )
                    .map_err(display)?;
                return Ok(staged);
            }
            let config = Config::parse(config)?;
            let host = context.host.ok_or("Host capabilities are unavailable")?;
            let repository =
                Repository::new(host.storage.clone(), &identity.entry_id).map_err(display)?;
            let controller = Controller::open(
                repository,
                config.timezone()?,
                jiff::Timestamp::now().as_millisecond(),
            )
            .await
            .map_err(display)?
            .with_misfire(config.misfire);
            let backend = Arc::new(backend::Backend { host });
            let (handle, owner) = crate::owner::start(
                controller,
                backend.clone(),
                Arc::new(SystemClock),
                context.lifecycle.stopping().map_err(display)?,
            );
            context
                .lifecycle
                .spawn("scheduled tasks", owner)
                .map_err(display)?;
            staged
                .insert(
                    identity.entry_id,
                    Arc::new(handle.clone()) as Arc<dyn maka_plugins::background::BackgroundWork>,
                )
                .map_err(display)?;
            let service = Service {
                context: context.lifecycle,
                handle,
                backend,
            };
            staged
                .insert("ScheduledTask", tools::register(service.clone())?)
                .map_err(display)?;
            remote::publish(service, &bundle, &mut staged)?;
            context
                .services
                .provide(CLIENT_SERVICE, Arc::new(ClientSupport(bundle)))
                .map_err(display)?;
            Ok(staged)
        })
    }
}
#[derive(Clone)]
struct Service {
    context: Context,
    handle: Handle,
    backend: Arc<backend::Backend>,
}
impl Service {
    fn query(&self, query: Query) -> Result<QueryResult, crate::Error> {
        let _lease = self.context.admit().map_err(|_| crate::Error::Closed)?;
        self.handle.query(query)
    }
    async fn mutate(
        &self,
        mutation: Mutation,
        origin: Origin,
    ) -> Result<MutationResult, crate::Error> {
        let _lease = self.context.admit().map_err(|_| crate::Error::Closed)?;
        self.handle.mutate(mutation, origin).await
    }
}
fn display(error: impl ToString) -> String {
    error.to_string()
}
