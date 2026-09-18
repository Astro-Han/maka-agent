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

use std::collections::BTreeSet;

use super::{Composition, Entry, Isolation, Operation, Scope};
use crate::{Error, identifier, name};

impl Operation {
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Insert {
                root_id,
                parent_id,
                entry,
                ..
            } => {
                if let Some(Scope::Session(id)) = root_id {
                    name(id)?;
                }
                if let Some(parent) = parent_id {
                    identifier(parent)?;
                }
                validate(std::slice::from_ref(entry), 0, &mut BTreeSet::new())
            }
            Self::Update { entry_id, patch } => {
                identifier(entry_id)?;
                let mut entry = Entry::new(entry_id.clone())?;
                patch.apply(&mut entry);
                validate(std::slice::from_ref(&entry), 0, &mut BTreeSet::new())
            }
            Self::Move {
                entry_id,
                parent_id,
                ..
            } => {
                identifier(entry_id)?;
                if let Some(parent) = parent_id {
                    identifier(parent)?;
                }
                Ok(())
            }
            Self::Remove { entry_id } => identifier(entry_id),
        }
    }
}

impl Composition {
    pub fn validate(&self) -> Result<(), Error> {
        let mut ids = BTreeSet::new();
        for (scope, entries) in &self.roots {
            if let Scope::Session(session) = scope {
                name(session)?;
            }
            validate(entries, 0, &mut ids)?;
        }
        Ok(())
    }
}

fn validate<'a>(
    entries: &'a [Entry],
    depth: usize,
    ids: &mut BTreeSet<&'a str>,
) -> Result<(), Error> {
    if depth > 64 {
        return Err(Error::Invalid("entry nesting exceeds 64".into()));
    }
    for entry in entries {
        identifier(&entry.id)?;
        config(&entry.config)?;
        if !ids.insert(&entry.id) {
            return Err(Error::DuplicateEntry(entry.id.clone()));
        }
        if ids.len() > 4096 {
            return Err(Error::Invalid("composition exceeds 4096 entries".into()));
        }
        if let Some(package) = &entry.package_id {
            identifier(package)?;
        }
        for dependency in entry.inject.names() {
            crate::services::validate_name(dependency)?;
        }
        for service in entry.isolate.keys().chain(entry.intercept.keys()) {
            crate::services::validate_name(service)?;
        }
        for isolation in entry.isolate.values() {
            match isolation {
                Isolation::Private(true) => {}
                Isolation::Private(false) => {
                    return Err(Error::Invalid("isolation must be true or a label".into()));
                }
                Isolation::Named(label) => name(label)?,
            }
        }
        validate(&entry.children, depth + 1, ids)?;
    }
    Ok(())
}

fn config(value: &serde_json::Value) -> Result<(), Error> {
    let mut pending = vec![(value, 0)];
    let mut nodes = 0;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if nodes > 8192 || depth > 64 {
            return Err(Error::Invalid("plugin config is too large".into()));
        }
        match value {
            serde_json::Value::Array(items) => {
                pending.extend(items.iter().map(|value| (value, depth + 1)))
            }
            serde_json::Value::Object(items) => {
                pending.extend(items.values().map(|value| (value, depth + 1)))
            }
            _ => {}
        }
    }
    if serde_json::to_vec(value)
        .map_err(|error| Error::Invalid(error.to_string()))?
        .len()
        > 64 * 1024
    {
        return Err(Error::Invalid("plugin config exceeds 64 KiB".into()));
    }
    Ok(())
}
