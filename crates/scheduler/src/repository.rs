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

//! Domain persistence uses the normal plugin CAS store; no second database owner.
use crate::{Error, invalid, plan::Plan};
use maka_plugins::storage::{Data, Mutation, Record, Store};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

const MAX_TASKS: usize = 256;
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Index {
    schema_version: u32,
    ids: Vec<String>,
}

pub struct Saved {
    pub revision: u64,
    pub plan: Plan,
}
pub struct Catalog {
    pub revision: Option<u64>,
    pub plans: BTreeMap<String, Saved>,
}
pub struct Repository {
    storage: Arc<dyn Store>,
    prefix: String,
}
impl Repository {
    pub fn new(storage: Arc<dyn Store>, entry_id: &str) -> Result<Self, Error> {
        maka_plugins::composition::Entry::new(entry_id)
            .map_err(|error| invalid(error.to_string()))?;
        Ok(Self {
            storage,
            prefix: format!("scheduled-tasks:{}:{entry_id}:", entry_id.len()),
        })
    }
    fn index(&self) -> String {
        format!("{}index", self.prefix)
    }
    fn key(&self, id: &str) -> String {
        format!("{}task:{id}", self.prefix)
    }
    pub async fn load(&self) -> Result<Catalog, Error> {
        // A concurrent writer may change the index between reads. Reject that
        // snapshot rather than return a mixed catalog under an old revision.
        let index = self.storage.read(self.index()).await?;
        let Some(index) = index else {
            return Ok(Catalog {
                revision: None,
                plans: BTreeMap::new(),
            });
        };
        let value: Index = decode(&index)?;
        if value.schema_version != 1 || value.ids.len() > MAX_TASKS {
            return Err(invalid(
                "unsupported scheduled-task schema or catalog limit",
            ));
        }
        let mut records = Vec::with_capacity(value.ids.len());
        for id in value.ids {
            let record = self.storage.read(self.key(&id)).await?;
            records.push((id, record));
        }
        let current = self.storage.read(self.index()).await?;
        if current.as_ref().map(|record| record.revision) != Some(index.revision) {
            return Err(maka_plugins::storage::StoreError::Conflict {
                expected: index.revision.to_string(),
                actual: current
                    .map_or_else(|| "missing".into(), |record| record.revision.to_string()),
            }
            .into());
        }
        let mut plans = BTreeMap::new();
        for (id, record) in records {
            let record =
                record.ok_or_else(|| invalid("scheduled-task index points to missing data"))?;
            let plan: Plan = decode(&record)?;
            plan.validate()?;
            if plan.task.id != id
                || plans
                    .insert(
                        id,
                        Saved {
                            revision: record.revision,
                            plan,
                        },
                    )
                    .is_some()
            {
                return Err(invalid("scheduled-task index identity mismatch"));
            }
        }
        Ok(Catalog {
            revision: Some(index.revision),
            plans,
        })
    }
    /// Save task bytes and the catalog revision in one CAS batch. Losing the reply
    /// requires reload, never treating the previous in-memory copy as authority.
    pub async fn save(&self, catalog: &mut Catalog, plan: Plan) -> Result<(), Error> {
        plan.validate()?;
        let id = plan.task.id.clone();
        let previous = catalog.plans.get(&id).map(|saved| saved.revision);
        let mut ids = catalog.plans.keys().cloned().collect::<Vec<_>>();
        if previous.is_none() {
            if ids.len() >= MAX_TASKS {
                return Err(invalid("scheduled-task catalog is full"));
            }
            // Deleted keys retain revisions; a new task ID must not reuse them.
            if self.storage.read(self.key(&id)).await?.is_some() {
                return Err(invalid("task identity has already been used"));
            }
            ids.push(id.clone());
            ids.sort();
        }
        let records = self
            .storage
            .batch(vec![
                Mutation {
                    key: self.index(),
                    expected_revision: catalog.revision,
                    data: present(&Index {
                        schema_version: 1,
                        ids,
                    })?,
                },
                Mutation {
                    key: self.key(&id),
                    expected_revision: previous,
                    data: present(&plan)?,
                },
            ])
            .await?;
        if records.len() != 2 {
            return Err(invalid("storage returned an invalid batch receipt"));
        }
        catalog.revision = Some(records[0].revision);
        catalog.plans.insert(
            id,
            Saved {
                revision: records[1].revision,
                plan,
            },
        );
        Ok(())
    }
    pub async fn remove(&self, catalog: &mut Catalog, id: &str) -> Result<(), Error> {
        let saved = catalog
            .plans
            .get(id)
            .ok_or_else(|| invalid("scheduled task does not exist"))?;
        if saved.plan.pending.is_some() && !saved.plan.waiting_notification() {
            return Err(invalid("trigger must settle before deleting its task"));
        }
        let ids = catalog
            .plans
            .keys()
            .filter(|key| key.as_str() != id)
            .cloned()
            .collect();
        let records = self
            .storage
            .batch(vec![
                Mutation {
                    key: self.index(),
                    expected_revision: catalog.revision,
                    data: present(&Index {
                        schema_version: 1,
                        ids,
                    })?,
                },
                Mutation {
                    key: self.key(id),
                    expected_revision: Some(saved.revision),
                    data: Data::Deleted,
                },
            ])
            .await?;
        if records.len() != 2 {
            return Err(invalid("storage returned an invalid batch receipt"));
        }
        catalog.revision = Some(records[0].revision);
        catalog.plans.remove(id);
        Ok(())
    }
}
fn decode<T: serde::de::DeserializeOwned>(record: &Record) -> Result<T, Error> {
    let value = record
        .data
        .value()
        .ok_or_else(|| invalid("unexpected scheduled-task tombstone"))?;
    serde_json::from_value(value.clone()).map_err(|error| invalid(error.to_string()))
}
fn present(value: &impl Serialize) -> Result<Data, Error> {
    serde_json::to_value(value)
        .map(Data::Present)
        .map_err(|error| invalid(error.to_string()))
}
