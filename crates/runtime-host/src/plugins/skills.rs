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

use super::Setup;
use futures_util::future::BoxFuture;
use maka_plugins::{
    composition::{Entry, Operation, Scope},
    kernel::Definition,
};
use maka_skills::plugin::{Builtin, ID, PreferenceSnapshot, PreferenceStore};
use std::{path::PathBuf, sync::Arc};

struct Preferences(Arc<maka_config::ConfigurationStore>);
impl PreferenceStore for Preferences {
    fn compare_exchange(
        &self,
        expected_revision: u64,
        reference: String,
        preference: maka_skills::Preference,
    ) -> BoxFuture<'_, Result<bool, String>> {
        Box::pin(async move {
            self.0
                .set_skill_preference(expected_revision, reference, preference)
                .await
                .map(|outcome| {
                    matches!(
                        outcome,
                        maka_config::skills::PreferenceUpdate::Committed { .. }
                    )
                })
                .map_err(|error| error.to_string())
        })
    }
    fn read(&self) -> BoxFuture<'_, Result<PreferenceSnapshot, String>> {
        Box::pin(async move {
            let snapshot = self
                .0
                .skill_preferences()
                .await
                .map_err(|error| error.to_string())?;
            Ok(PreferenceSnapshot {
                revision: snapshot.revision,
                entries: snapshot.entries,
            })
        })
    }
}

pub(crate) fn install(
    setup: &mut Setup,
    configuration: Arc<maka_config::ConfigurationStore>,
    state_root: PathBuf,
    home: Option<PathBuf>,
    executions: &Arc<crate::execution::Executions>,
) -> Result<(), maka_plugins::Error> {
    let catalog = &executions.plugin_catalog;
    if setup.builtins.contains_key(ID) || setup.layers.contains_key(ID) {
        return Err(maka_plugins::Error::Invalid(
            "built-in Skills identity is reserved".into(),
        ));
    }
    catalog.host_only::<maka_skills::plugin::Skills>()?;
    catalog.reserve_for::<maka_skills::plugin::Skills>(ID, ID)?;
    catalog.host_only::<maka_plugins::input::InputPreparation>()?;
    catalog.reserve_for::<maka_plugins::input::InputPreparation>(ID, ID)?;
    for name in ["Skill", "SkillSearch"] {
        catalog.reserve_for::<maka_tools::plugins::PluginTool>(name, ID)?;
    }
    setup.builtins.insert(
        ID.into(),
        Arc::new(Definition {
            id: ID.into(),
            revision: env!("CARGO_PKG_VERSION").into(),
            dependencies: vec![],
            inject: vec![],
            plugin: Arc::new(Builtin {
                state_root,
                home,
                preferences: Arc::new(Preferences(configuration)),
                client: Some(maka_skills::plugin::remote::ClientSupport {
                    bundle: maka_plugins::client::Bundle::builtin(
                        ID,
                        env!("CARGO_PKG_VERSION"),
                        include_str!(concat!(env!("OUT_DIR"), "/skills-client.js")),
                    )?,
                    sessions: Arc::new(crate::execution::SessionViews(Arc::downgrade(executions))),
                    workspaces: Arc::new(crate::execution::SessionViews(Arc::downgrade(
                        executions,
                    ))),
                }),
            }),
        }),
    );
    let mut entry = Entry::new(ID)?;
    entry.package_id = Some(ID.into());
    let mut client = Entry::new("maka.skills.ui")?;
    client.package_id = Some(ID.into());
    client.inject = maka_plugins::composition::Injection::Names(vec![
        maka_skills::plugin::remote::CLIENT_SERVICE.into(),
    ]);
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
