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

use super::{Definitions, Plan};
use crate::{
    Error,
    composition::{Composition, Entry, Scope},
    identifier,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Validation {
    Strict,
    Recovery,
}

pub(super) fn prepare(
    composition: &Composition,
    definitions: &Definitions,
    mode: Validation,
) -> Result<(BTreeMap<String, Plan>, Vec<String>), Error> {
    composition.validate()?;
    let mut done = BTreeSet::new();
    let mut package_errors = BTreeMap::new();
    for (id, definition) in definitions {
        identifier(id)?;
        if id != &definition.id || definition.revision.is_empty() {
            return Err(Error::Invalid("invalid package definition identity".into()));
        }
        for service in &definition.inject {
            crate::services::validate_name(service)?;
        }
        if let Err(error) = visit(id, definitions, &mut BTreeSet::new(), &mut done) {
            if mode == Validation::Strict {
                return Err(error);
            }
            package_errors.insert(id.as_str(), error.to_string());
        }
    }
    let mut builder = Builder {
        definitions,
        mode,
        package_errors,
        plans: BTreeMap::new(),
        order: Vec::new(),
    };
    for (scope, entries) in &composition.roots {
        builder.flatten(scope, entries, None, false)?;
    }
    let active: BTreeSet<_> = builder
        .plans
        .values()
        .filter(|plan| !plan.disabled)
        .filter_map(|plan| {
            plan.entry
                .package_id
                .as_ref()
                .map(|package| (plan.scope.clone(), package.clone()))
        })
        .collect();
    for plan in builder.plans.values_mut().filter(|plan| !plan.disabled) {
        let Some(package) = plan.entry.package_id.as_ref() else {
            continue;
        };
        for dependency in &definitions[package].dependencies {
            if !active.contains(&(plan.scope.clone(), dependency.clone())) {
                let error = format!("package {package} requires {dependency} in the same root");
                if mode == Validation::Strict {
                    return Err(Error::Invalid(error));
                }
                plan.problem = Some(error);
            }
        }
    }
    Ok((builder.plans, builder.order))
}

struct Builder<'a> {
    definitions: &'a Definitions,
    mode: Validation,
    package_errors: BTreeMap<&'a str, String>,
    plans: BTreeMap<String, Plan>,
    order: Vec<String>,
}

impl Builder<'_> {
    fn flatten(
        &mut self,
        scope: &Scope,
        entries: &[Entry],
        parent: Option<&str>,
        ancestor_disabled: bool,
    ) -> Result<(), Error> {
        for entry in entries {
            let disabled = ancestor_disabled || entry.disabled;
            let mut problem = None;
            let revision = if let Some(package) = &entry.package_id {
                let definition = self
                    .definitions
                    .get(package)
                    .ok_or_else(|| Error::Invalid(format!("missing package: {package}")))?;
                if !disabled {
                    let validation = if !definition.plugin.supports_scope(scope) {
                        Err(Error::Invalid(format!(
                            "package {package} has no entrypoint for {}",
                            String::from(scope.clone())
                        )))
                    } else {
                        match self.package_errors.get(package.as_str()) {
                            Some(error) => Err(Error::Invalid(error.clone())),
                            None => definition.plugin.validate(scope, &entry.config),
                        }
                    };
                    if let Err(error) = validation {
                        if self.mode == Validation::Strict {
                            return Err(error);
                        }
                        problem = Some(error.to_string());
                    }
                }
                Some(definition.revision.clone())
            } else {
                None
            };
            let mut shallow = entry.clone();
            shallow.children.clear();
            self.plans.insert(
                entry.id.clone(),
                Plan {
                    scope: scope.clone(),
                    parent: parent.map(str::to_owned),
                    entry: shallow,
                    disabled,
                    revision,
                    problem,
                },
            );
            self.order.push(entry.id.clone());
            self.flatten(scope, &entry.children, Some(&entry.id), disabled)?;
        }
        Ok(())
    }
}

fn visit<'a>(
    id: &'a str,
    definitions: &'a Definitions,
    path: &mut BTreeSet<&'a str>,
    done: &mut BTreeSet<&'a str>,
) -> Result<(), Error> {
    if done.contains(id) {
        return Ok(());
    }
    if !path.insert(id) {
        return Err(Error::Invalid(format!("cyclic package dependency: {id}")));
    }
    let definition = definitions
        .get(id)
        .ok_or_else(|| Error::Invalid(format!("missing package dependency: {id}")))?;
    for dependency in &definition.dependencies {
        visit(dependency, definitions, path, done)?;
    }
    path.remove(id);
    done.insert(id);
    Ok(())
}
