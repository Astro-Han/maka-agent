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

use crate::Error;
use maka_plugins::storage::{Data, Mutation, Store};
use maka_runtime::execution::ThinkingLevel;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    LocalRead,
    WebResearch,
    Implementation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub profile: Profile,
    pub connection_slug: String,
    pub model: String,
    pub thinking_level: Option<ThinkingLevel>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub revision: Option<u64>,
    pub presets: Vec<Preset>,
}

/// Presets are plugin data, not Host execution policy.
pub struct Settings {
    store: Arc<dyn Store>,
}
impl Settings {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    pub async fn read(&self) -> Result<Snapshot, Error> {
        let record = self.store.read("presets".into()).await?;
        let presets: Vec<Preset> = record
            .as_ref()
            .and_then(|record| record.data.value())
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|error| Error::Persistence(error.to_string()))?
            .unwrap_or_default();
        validate(&presets)?;
        Ok(Snapshot {
            revision: record.map(|record| record.revision),
            presets,
        })
    }

    pub async fn replace(&self, snapshot: Snapshot) -> Result<Snapshot, Error> {
        validate(&snapshot.presets)?;
        let data = serde_json::to_value(&snapshot.presets)
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let committed = self
            .store
            .batch(vec![Mutation {
                key: "presets".into(),
                expected_revision: snapshot.revision,
                data: Data::Present(data),
            }])
            .await?;
        Ok(Snapshot {
            revision: Some(committed[0].revision),
            presets: snapshot.presets,
        })
    }
}

fn validate(presets: &[Preset]) -> Result<(), Error> {
    if presets.len() > 64 {
        return Err(Error::Invalid(
            "At most 64 agent presets are supported".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    for preset in presets {
        crate::identity(&preset.id)?;
        if preset.id.len() > 128
            || !ids.insert(&preset.id)
            || !preset
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
            || preset.name.trim().is_empty()
            || preset.name.len() > 512
            || preset.description.len() > 4000
        {
            return Err(Error::Invalid("Invalid or duplicate agent preset".into()));
        }
        maka_plugins::llm::Selection::Named {
            connection_slug: preset.connection_slug.clone(),
            model: preset.model.clone(),
        }
        .validate()
        .map_err(|error| Error::Invalid(error.to_string()))?;
    }
    Ok(())
}
