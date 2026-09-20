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
use maka_plugins::{
    composition::{Entry, Operation, Scope},
    kernel::Definition,
};
use maka_skills::plugin::{Builtin, ID};
use std::{path::PathBuf, sync::Arc};

pub(crate) fn install(
    setup: &mut Setup,
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
    catalog.host_only::<maka_plugins::input::InputPreparation>()?;
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
                client: Some(maka_plugins::client::Bundle::builtin(
                    ID,
                    env!("CARGO_PKG_VERSION"),
                    include_str!(concat!(env!("OUT_DIR"), "/skills-client.js")),
                )?),
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
