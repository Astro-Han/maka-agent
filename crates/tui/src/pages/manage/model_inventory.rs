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

use crate::pages::connections::Row;
use maka_protocol::configuration::{
    ConnectionCatalogCursor as Cursor, ConnectionCatalogQueryInput as Query, ModelOverride,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

pub struct Model {
    pub id: String,
    pub name: String,
    pub default_context: Option<u64>,
    pub default_input: Option<u64>,
}

pub struct Catalog {
    pub basis: Arc<Row>,
    source: Query,
    query: Option<Query>,
    index: Option<usize>,
    expected: Option<usize>,
    capture_overrides: bool,
    pub overrides: BTreeMap<String, ModelOverride>,
    enabled: Vec<String>,
    pub rows: Vec<Model>,
    pub ready: bool,
    pub error: Option<&'static str>,
}
impl Catalog {
    pub fn new(basis: Arc<Row>, source: Query) -> Self {
        let index = match &source {
            Query::Start => None,
            Query::Continue {
                cursor: Cursor::Connection { connection_index },
                ..
            } => Some(*connection_index),
            _ => unreachable!("connection overview basis"),
        };
        Self {
            basis,
            index,
            query: Some(source.clone()),
            source,
            expected: None,
            capture_overrides: false,
            overrides: BTreeMap::new(),
            enabled: vec![],
            rows: vec![],
            ready: false,
            error: None,
        }
    }
    pub fn query(&mut self) -> Option<Query> {
        self.query.take()
    }
    pub fn with_overrides(mut self) -> Self {
        self.capture_overrides = true;
        self
    }
    pub fn retry(&mut self) {
        let capture = self.capture_overrides;
        *self = Self::new(self.basis.clone(), self.source.clone());
        self.capture_overrides = capture;
    }
    pub fn complete(&mut self, result: Result<Value, String>) {
        let Ok(page) = result else {
            self.error = Some("session-model-load-failed");
            return;
        };
        if page["kind"] == "revision_changed" {
            self.error = Some("connection-edit-conflict");
            return;
        }
        if !self.accept(&page) {
            self.rows.clear();
            self.error = Some("connection-edit-conflict");
        }
    }
    fn accept(&mut self, page: &Value) -> bool {
        // An acknowledged row can open while the overview is refreshing. Find
        // its header in a fresh snapshot, without rebasing the captured row CAS
        // or walking another connection's entire model inventory.
        if self.index.is_none() {
            let mut next = None;
            for item in page["items"].as_array().expect("checked page") {
                if item["kind"] != "connection" {
                    continue;
                }
                let index = item["connectionIndex"].as_u64().unwrap() as usize;
                if item["connectionId"] == self.basis.id {
                    self.index = Some(index);
                    break;
                }
                next = Some(index + 1);
            }
            if self.index.is_none() {
                let Some(next) =
                    next.filter(|n| (*n as u64) < page["connectionCount"].as_u64().unwrap())
                else {
                    return false;
                };
                self.query = Some(Query::Continue {
                    revision: page["revision"].as_u64().unwrap(),
                    cursor: Cursor::Connection {
                        connection_index: next,
                    },
                });
                return true;
            }
        }
        let index = self.index.unwrap();
        for item in page["items"].as_array().expect("checked page") {
            if item["connectionIndex"].as_u64().unwrap() < index as u64 {
                continue;
            }
            if item["connectionIndex"] != index {
                break;
            }
            match item["kind"].as_str().expect("checked item") {
                "connection" => {
                    if self.expected.is_some()
                        || item["connectionId"] != self.basis.id
                        || item["revision"] != self.basis.revision
                    {
                        return false;
                    }
                    self.expected = item["catalogEntryCount"].as_u64().map(|n| n as usize);
                }
                "enabled_model_id" => {
                    if item["itemIndex"] != self.enabled.len() || self.enabled.len() >= 512 {
                        return false;
                    }
                    self.enabled.push(item["modelId"].as_str().unwrap().into());
                }
                "catalog_entry" => {
                    if item["itemIndex"] != self.rows.len()
                        || self.rows.len()
                            >= maka_protocol::configuration_pages::MAX_CATALOG_ENTRIES as usize
                    {
                        return false;
                    }
                    let entry = &item["entry"];
                    let id = entry["id"].as_str().unwrap();
                    if self.capture_overrides
                        && let Some(profile) = item.get("modelOverride")
                    {
                        let Ok(profile) = serde_json::from_value(profile.clone()) else {
                            return false;
                        };
                        if self.overrides.len() >= 2048
                            || self.overrides.insert(id.into(), profile).is_some()
                        {
                            return false;
                        }
                    }
                    self.rows.push(Model {
                        id: id.into(),
                        default_context: entry["defaultContextWindow"].as_u64(),
                        default_input: entry["defaultInputLimit"].as_u64(),
                        name: entry["displayName"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or(id)
                            .into(),
                    });
                }
                _ => {} // Raw inventory is already projected by the Host into catalog entries.
            }
        }
        if self.expected == Some(self.rows.len())
            && self.enabled.len() == self.basis.model_ids.len()
        {
            if self.enabled != self.basis.model_ids {
                return false;
            }
            let mut unique: HashSet<_> = self.rows.iter().map(|row| row.id.clone()).collect();
            if unique.len() != self.rows.len() {
                return false;
            }
            for id in &self.basis.model_ids {
                if unique.insert(id.clone()) {
                    self.rows.push(Model {
                        id: id.clone(),
                        name: id.clone(),
                        default_context: None,
                        default_input: None,
                    });
                }
            }
            self.rows.sort_by(|a, b| {
                (!self.basis.model_ids.contains(&a.id), &a.id)
                    .cmp(&(!self.basis.model_ids.contains(&b.id), &b.id))
            });
            self.ready = true;
            return true;
        }
        let Ok(cursor) = serde_json::from_value::<Cursor>(page["nextCursor"].clone()) else {
            return false;
        };
        if page["nextCursor"]["connectionIndex"] != index {
            return false;
        }
        self.query = Some(Query::Continue {
            revision: page["revision"].as_u64().unwrap(),
            cursor,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fresh_lookup_skips_other_inventory_without_rebasing_the_captured_connection() {
        let row = Arc::new(Row {
            id: "target".into(),
            name: "Target".into(),
            slug: "target".into(),
            provider: "openai-compatible".into(),
            enabled: true,
            enabled_models: 0,
            model_ids: vec![],
            revision: 7,
            base_url: None,
            default_model: None,
        });
        for (revision, count, ready) in [(7, 2, true), (8, 2, false), (7, 1, false)] {
            let mut catalog = Catalog::new(row.clone(), Query::Start);
            assert_eq!(catalog.query(), Some(Query::Start));
            catalog.complete(Ok(
                json!({"kind":"page","revision":20,"connectionCount":count,
                "items":[{"kind":"connection","connectionIndex":0,"connectionId":"other"}],
                "nextCursor":{"part":"model","connectionIndex":0,"itemIndex":50}}),
            ));
            if count == 2 {
                assert_eq!(
                    catalog.query(),
                    Some(Query::Continue {
                        revision: 20,
                        cursor: Cursor::Connection {
                            connection_index: 1
                        }
                    })
                );
                catalog.complete(Ok(json!({"kind":"page","revision":20,"connectionCount":2,
                    "items":[{"kind":"connection","connectionIndex":1,"connectionId":"target","revision":revision,"catalogEntryCount":0}],"nextCursor":null})));
            }
            assert_eq!(catalog.ready, ready);
            assert_eq!(catalog.error.is_some(), !ready);
            assert_eq!(catalog.basis.revision, 7);
        }
    }
}
