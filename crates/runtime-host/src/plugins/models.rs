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
use maka_model::adapters::{Builtin, ID};
use maka_plugins::{
    composition::{Entry, Operation, Scope},
    kernel::{Definition, Plugin},
};
use std::sync::Arc;

pub(crate) fn install(
    setup: &mut Setup,
    runtime: maka_js_runtime::trusted::TrustedRuntime,
) -> Result<(), maka_plugins::Error> {
    let plugins: [(&str, Arc<dyn Plugin>); 3] = [
        (ID, Arc::new(Builtin(runtime))),
        (
            maka_providers::codex::ID,
            Arc::new(maka_providers::codex::Codex::default()),
        ),
        (
            maka_providers::api::ID,
            Arc::new(maka_providers::api::ApiProviders),
        ),
    ];
    if plugins
        .iter()
        .any(|(id, _)| setup.builtins.contains_key(*id) || setup.layers.contains_key(*id))
    {
        return Err(maka_plugins::Error::Invalid(
            "built-in model plugin identity is reserved".into(),
        ));
    }
    for (id, plugin) in plugins {
        setup.builtins.insert(
            id.into(),
            Arc::new(Definition {
                id: id.into(),
                revision: env!("CARGO_PKG_VERSION").into(),
                dependencies: vec![],
                inject: vec![],
                plugin,
            }),
        );
        let mut entry = Entry::new(id)?;
        entry.package_id = Some(id.into());
        setup.layers.insert(
            id.into(),
            vec![Operation::Insert {
                root_id: Some(Scope::Profile),
                parent_id: None,
                position: None,
                entry,
            }],
        );
    }
    Ok(())
}
