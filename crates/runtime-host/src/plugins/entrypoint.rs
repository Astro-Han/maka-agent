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

use futures_util::future::BoxFuture;
use maka_plugins::{
    client::{Bundle, Client},
    composition::Scope,
    contributions::Staged,
    kernel::{Plugin, PluginContext},
};
use serde_json::Value;
use std::sync::Arc;

/// desktop-ui publishes bytes only. It never evaluates the package's Host code.
pub(super) struct Entrypoint {
    pub host: Option<Arc<dyn Plugin>>,
    pub client: Option<Arc<Bundle>>,
}
impl Plugin for Entrypoint {
    fn validate(&self, scope: &Scope, config: &Value) -> Result<(), maka_plugins::Error> {
        match scope {
            Scope::DesktopUi => Ok(()),
            _ => match &self.host {
                Some(host) => host.validate(scope, config),
                None => Err(maka_plugins::Error::Invalid(
                    "package has no Host entrypoint".into(),
                )),
            },
        }
    }
    fn supports_scope(&self, scope: &Scope) -> bool {
        match scope {
            Scope::DesktopUi => self.client.is_some(),
            _ => self
                .host
                .as_ref()
                .is_some_and(|host| host.supports_scope(scope)),
        }
    }
    fn activate(
        &self,
        context: PluginContext,
        config: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        let identity = match context.lifecycle.identity() {
            Ok(identity) => identity,
            Err(error) => return Box::pin(async move { Err(error.to_string()) }),
        };
        if identity.scope != Scope::DesktopUi {
            return match &self.host {
                Some(host) => host.activate(context, config),
                None => Box::pin(async { Err("package has no Host entrypoint".into()) }),
            };
        }
        let bundle = self.client.clone();
        Box::pin(async move {
            let bundle = bundle.ok_or("package has no client entrypoint")?;
            let mut staged = Staged::default();
            staged
                .insert(identity.entry_id, Client { bundle, config })
                .map_err(|error| error.to_string())?;
            Ok(staged)
        })
    }
}
