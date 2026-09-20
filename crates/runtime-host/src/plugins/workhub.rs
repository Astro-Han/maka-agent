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
    client::Bundle,
    composition::{Entry, Operation, Scope},
    kernel::Definition,
};
use maka_workhub::{Builtin, ID};
use std::sync::Arc;

pub(crate) fn install(setup: &mut Setup) -> Result<(), maka_plugins::Error> {
    if setup.builtins.contains_key(ID) || setup.layers.contains_key(ID) {
        return Err(maka_plugins::Error::Invalid(
            "built-in WorkHub identity is reserved".into(),
        ));
    }
    let bundle = Bundle::builtin(
        ID,
        env!("CARGO_PKG_VERSION"),
        include_str!(concat!(env!("OUT_DIR"), "/workhub-client.js")),
    )?;
    setup.builtins.insert(
        ID.into(),
        Arc::new(Definition {
            id: ID.into(),
            revision: env!("CARGO_PKG_VERSION").into(),
            dependencies: vec![],
            inject: vec![],
            plugin: Arc::new(Builtin { bundle }),
        }),
    );
    let mut entry = Entry::new(ID)?;
    entry.package_id = Some(ID.into());
    let mut client = Entry::new("maka.workhub.ui")?;
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
