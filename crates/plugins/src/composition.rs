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

mod ledger;
mod project;
mod validate;
pub use ledger::Ledger;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

pub use maka_runtime::scope::Scope;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Injection {
    Names(Vec<String>),
    Configured(BTreeMap<String, Value>),
}

impl Default for Injection {
    fn default() -> Self {
        Self::Names(Vec::new())
    }
}

impl Injection {
    pub fn names(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Self::Names(names) => Box::new(names.iter().map(String::as_str)),
            Self::Configured(values) => Box::new(values.keys().map(String::as_str)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Isolation {
    Private(bool),
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub inject: Injection,
    #[serde(default)]
    pub isolate: BTreeMap<String, Isolation>,
    #[serde(default)]
    pub intercept: BTreeMap<String, Value>,
    #[serde(default)]
    pub children: Vec<Entry>,
}

impl Entry {
    pub fn new(id: impl Into<String>) -> Result<Self, Error> {
        let id = id.into();
        crate::identifier(&id)?;
        Ok(Self {
            id,
            package_id: None,
            config: Value::Null,
            disabled: false,
            inject: Injection::default(),
            isolate: BTreeMap::new(),
            intercept: BTreeMap::new(),
            children: Vec::new(),
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntryPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub package_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub config: Option<Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub disabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub inject: Option<Injection>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub isolate: Option<BTreeMap<String, Isolation>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub intercept: Option<BTreeMap<String, Value>>,
}

impl EntryPatch {
    pub(crate) fn merge(&mut self, next: &Self) {
        if next.package_id.is_some() {
            self.package_id = next.package_id.clone();
        }
        if next.config.is_some() {
            self.config = next.config.clone();
        }
        if next.disabled.is_some() {
            self.disabled = next.disabled;
        }
        if next.inject.is_some() {
            self.inject = next.inject.clone();
        }
        if next.isolate.is_some() {
            self.isolate = next.isolate.clone();
        }
        if next.intercept.is_some() {
            self.intercept = next.intercept.clone();
        }
    }

    pub(crate) fn apply(&self, entry: &mut Entry) {
        if let Some(value) = &self.package_id {
            entry.package_id = Some(value.clone());
        }
        if let Some(value) = &self.config {
            entry.config = value.clone();
        }
        if let Some(value) = self.disabled {
            entry.disabled = value;
        }
        if let Some(value) = &self.inject {
            entry.inject = value.clone();
        }
        if let Some(value) = &self.isolate {
            entry.isolate = value.clone();
        }
        if let Some(value) = &self.intercept {
            entry.intercept = value.clone();
        }
    }
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Operation {
    Insert {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        root_id: Option<Scope>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        entry: Entry,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<usize>,
    },
    Update {
        entry_id: String,
        patch: EntryPatch,
    },
    Move {
        entry_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<usize>,
    },
    Remove {
        entry_id: String,
    },
}

/// Derived state only. The store persists package layers and user operations.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Composition {
    pub roots: BTreeMap<Scope, Vec<Entry>>,
}
