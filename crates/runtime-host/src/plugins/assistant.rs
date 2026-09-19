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
mod prompt;

use super::Setup;
use futures_util::future::BoxFuture;
use maka_plugins::{
    composition::{Entry, Operation, Scope},
    contributions::{Catalog, Staged},
    fiber::Context,
    kernel::{Definition, Plugin, PluginContext},
    prompt::{Provider, Request, Section, SectionMode, Text, TextFuture},
    session::{Behavior, Preparation, SessionBehavior},
};
use maka_runtime::configuration::policy::RuntimePolicySnapshot;
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};

const ID: &str = "maka.assistant";

trait Preferences: Send + Sync {
    fn read(&self) -> BoxFuture<'_, Result<RuntimePolicySnapshot, String>>;
}
struct Settings(Arc<maka_config::ConfigurationStore>);
impl Preferences for Settings {
    fn read(&self) -> BoxFuture<'_, Result<RuntimePolicySnapshot, String>> {
        Box::pin(async { self.0.runtime_policy().await.map_err(|e| e.to_string()) })
    }
}

pub(crate) fn install(
    setup: &mut Setup,
    configuration: Arc<maka_config::ConfigurationStore>,
    global: Option<PathBuf>,
    catalog: &Catalog,
) -> Result<(), maka_plugins::Error> {
    if setup.builtins.contains_key(ID) || setup.layers.contains_key(ID) {
        return Err(maka_plugins::Error::Invalid(
            "built-in assistant identity is reserved".into(),
        ));
    }
    catalog.reserve_for::<Section>(ID, ID)?;
    catalog.host_only::<SessionBehavior>()?;
    catalog.reserve_for::<SessionBehavior>("default", ID)?;
    setup.builtins.insert(
        ID.into(),
        Arc::new(Definition {
            id: ID.into(),
            revision: env!("CARGO_PKG_VERSION").into(),
            dependencies: vec![],
            inject: vec![],
            plugin: Arc::new(Builtin {
                preferences: Arc::new(Settings(configuration)),
                global,
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
struct Builtin {
    preferences: Arc<dyn Preferences>,
    global: Option<PathBuf>,
}
impl Plugin for Builtin {
    fn validate(&self, _: &Scope, config: &Value) -> Result<(), maka_plugins::Error> {
        if config.is_null() || config.as_object().is_some_and(|value| value.is_empty()) {
            Ok(())
        } else {
            Err(maka_plugins::Error::Invalid(
                "assistant has no instance configuration".into(),
            ))
        }
    }
    fn supports_scope(&self, scope: &Scope) -> bool {
        *scope == Scope::Profile
    }
    fn activate(
        &self,
        context: PluginContext,
        _: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        let assistant = Arc::new(Assistant {
            preferences: self.preferences.clone(),
            global: self.global.clone(),
            owner: context.lifecycle,
        });
        Box::pin(async move {
            let mut staged = Staged::default();
            staged
                .insert("default", SessionBehavior(assistant.clone()))
                .map_err(|e| e.to_string())?;
            staged
                .insert(
                    ID,
                    Section {
                        format: maka_plugins::prompt::Format::Plain,
                        order: i32::MIN,
                        mode: SectionMode::Append,
                        text: Text::Dynamic(assistant),
                    },
                )
                .map_err(|e| e.to_string())?;
            Ok(staged)
        })
    }
}
struct Assistant {
    preferences: Arc<dyn Preferences>,
    global: Option<PathBuf>,
    owner: Context,
}
impl Behavior for Assistant {
    fn prepare(&self, _: String) -> BoxFuture<'_, Result<Preparation, String>> {
        Box::pin(async { Ok(Preparation::default()) })
    }
}
impl Provider for Assistant {
    fn evaluate(&self, request: Request) -> TextFuture {
        let preferences = self.preferences.clone();
        let global = self.global.clone();
        let owner = self.owner.clone();
        Box::pin(async move {
            let guard = owner.admit()?;
            let snapshot = preferences
                .read()
                .await
                .map_err(maka_plugins::Error::Invalid)?;
            let prompt = prompt::resolve(snapshot, request.target.cwd().into(), global, guard)
                .await
                .map_err(|e| maka_plugins::Error::Invalid(e.to_string()))?;
            prompt
                .validate()
                .map_err(|e| maka_plugins::Error::Invalid(e.into()))?;
            Ok(Some(prompt.text))
        })
    }
}
