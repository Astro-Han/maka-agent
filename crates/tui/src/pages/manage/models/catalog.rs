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

use super::Choice;
use maka_protocol::configuration::{
    ConnectionCatalogCursor as Cursor, ConnectionCatalogQueryInput as Query,
};
use serde_json::Value;
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

#[derive(Clone)]
struct Header {
    index: u64,
    id: String,
    slug: String,
    name: String,
    enabled: bool,
    models: HashSet<String>,
}
#[derive(Clone, Default)]
struct Basis {
    cursor: Option<Cursor>,
    header: Option<Arc<Header>>,
}
pub struct Row {
    pub choice: Choice,
    pub name: String,
    pub connection: String,
    pub is_default: bool,
    pub thinking_levels: Vec<maka_protocol::session::ThinkingLevel>,
}
#[derive(Default)]
pub struct Catalog {
    pub rows: Vec<Row>,
    pub selected: Option<Choice>,
    pub loading: bool,
    pub error: bool,
    requested: bool,
    restart: bool,
    revision: Option<u64>,
    pub has_default: bool,
    basis: Basis,
    next: Option<Basis>,
    previous: VecDeque<Basis>,
}
impl Catalog {
    pub fn revision(&self) -> Option<u64> {
        self.ready().then_some(self.revision).flatten()
    }
    pub fn refresh(&mut self) {
        self.restart = true;
        self.requested = true;
    }
    pub fn ready(&self) -> bool {
        !self.loading && !self.requested && !self.error
    }
    pub fn can_previous(&self) -> bool {
        !self.previous.is_empty() && !self.loading && !self.requested
    }
    pub fn can_next(&self) -> bool {
        self.ready() && self.next.is_some()
    }
    pub fn change_page(&mut self, next: bool) {
        if next {
            if let Some(next) = self.next.clone() {
                if self.previous.len() == 128 {
                    self.previous.pop_front();
                }
                self.previous.push_back(self.basis.clone());
                self.basis = next;
                self.requested = true;
            }
        } else if let Some(previous) = self.previous.pop_back() {
            self.basis = previous;
            self.requested = true;
        }
    }
    pub fn query(&mut self) -> Option<Query> {
        if self.loading || !self.requested {
            return None;
        }
        if std::mem::take(&mut self.restart) {
            self.basis = Basis::default();
            self.revision = None;
            self.next = None;
            self.previous.clear();
        }
        self.loading = true;
        self.requested = false;
        self.error = false;
        Some(match (&self.revision, &self.basis.cursor) {
            (Some(revision), Some(cursor)) => Query::Continue {
                revision: *revision,
                cursor: cursor.clone(),
            },
            _ => Query::Start,
        })
    }
    pub fn complete(&mut self, result: Result<Value, String>) {
        self.loading = false;
        if self.restart {
            return;
        }
        let Ok(page) = result else {
            self.error = true;
            return;
        };
        if page["kind"] == "revision_changed" {
            self.refresh();
            return;
        }
        self.revision = page["revision"].as_u64();
        self.has_default = !page["defaultTarget"].is_null();
        let mut header = self.basis.header.clone();
        let mut rows = vec![];
        for item in page["items"].as_array().expect("checked catalog page") {
            let index = item["connectionIndex"].as_u64().expect("checked index");
            match item["kind"].as_str().expect("checked item") {
                "connection" => {
                    header = Some(Arc::new(Header {
                        index,
                        id: item["connectionId"].as_str().unwrap().into(),
                        slug: item["slug"].as_str().unwrap().into(),
                        name: item["name"].as_str().unwrap().into(),
                        enabled: item["enabled"].as_bool().unwrap(),
                        models: HashSet::new(),
                    }))
                }
                "enabled_model_id" => {
                    let Some(header) = header.as_mut().filter(|h| h.index == index) else {
                        self.error = true;
                        return;
                    };
                    Arc::make_mut(header)
                        .models
                        .insert(item["modelId"].as_str().unwrap().into());
                }
                "catalog_entry" => {
                    let Some(header) = header.as_ref().filter(|h| h.index == index) else {
                        self.error = true;
                        return;
                    };
                    let entry = &item["entry"];
                    let model = entry["id"].as_str().unwrap();
                    if header.enabled
                        && header.models.contains(model)
                        && entry["canUseAsChatDefault"] == true
                    {
                        rows.push(Row {
                            choice: Choice {
                                connection_id: header.id.clone(),
                                slug: header.slug.clone(),
                                model: model.into(),
                            },
                            name: entry["displayName"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .unwrap_or(model)
                                .into(),
                            connection: header.name.clone(),
                            is_default: entry["isDefault"] == true,
                            thinking_levels: serde_json::from_value(
                                entry["thinkingLevels"].clone(),
                            )
                            .unwrap_or_default(),
                        });
                    }
                }
                _ => {} // Raw discovery models and overrides are not additional selectable identities.
            }
        }
        self.next = if page["nextCursor"].is_null() {
            None
        } else {
            Some(Basis {
                cursor: Some(
                    serde_json::from_value(page["nextCursor"].clone()).expect("checked cursor"),
                ),
                header,
            })
        };
        // Inventory/disabled-only pages are scanned without accumulating them.
        if rows.is_empty()
            && let Some(next) = self.next.take()
        {
            self.basis = next;
            self.requested = true;
            return;
        }
        self.rows = rows;
        if !self
            .rows
            .iter()
            .any(|row| Some(&row.choice) == self.selected.as_ref())
        {
            self.selected = None;
        }
    }
    pub fn selection(&self) -> Option<&Row> {
        self.ready().then_some(())?;
        self.rows
            .iter()
            .find(|row| Some(&row.choice) == self.selected.as_ref())
    }
    pub fn move_selection(&mut self, down: bool) {
        let index = self
            .rows
            .iter()
            .position(|r| Some(&r.choice) == self.selected.as_ref())
            .map_or(0, |index| {
                if down {
                    (index + 1).min(self.rows.len().saturating_sub(1))
                } else {
                    index.saturating_sub(1)
                }
            });
        self.selected = self.rows.get(index).map(|r| r.choice.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page(items: Vec<Value>, next: Value) -> Value {
        json!({"kind":"page","revision":7,"items":items,"nextCursor":next})
    }
    fn header(index: u64, enabled: bool) -> Value {
        json!({"kind":"connection","connectionIndex":index,"connectionId":format!("id-{index}"),
            "slug":format!("slug-{index}"),"name":"Same connection name","enabled":enabled})
    }
    fn enabled(index: u64) -> Value {
        json!({"kind":"enabled_model_id","connectionIndex":index,"modelId":"same-model"})
    }
    fn entry(index: u64, id: &str, chat: bool) -> Value {
        json!({"kind":"catalog_entry","connectionIndex":index,"entry":{"id":id,
            "displayName":"Same model name","canUseAsChatDefault":chat}})
    }

    #[test]
    fn catalog_carries_connection_identity_across_inventory_pages_and_invalidates_stale_choices() {
        let mut catalog = Catalog::default();
        catalog.refresh();
        assert_eq!(catalog.query(), Some(Query::Start));
        assert!(catalog.query().is_none());
        let cursor = json!({"part":"enabled_model_id","connectionIndex":0,"itemIndex":0});
        catalog.complete(Ok(page(vec![header(0, true)], cursor)));
        assert!(
            !catalog.ready(),
            "inventory-only pages continue without showing an empty catalog"
        );
        assert!(matches!(
            catalog.query(),
            Some(Query::Continue {
                revision: 7,
                cursor: Cursor::EnabledModelId {
                    connection_index: 0,
                    item_index: 0
                }
            })
        ));
        catalog.complete(Ok(page(
            vec![enabled(0)],
            json!({"part":"catalog_entry","connectionIndex":0,"itemIndex":0}),
        )));
        assert!(matches!(
            catalog.query(),
            Some(Query::Continue {
                revision: 7,
                cursor: Cursor::CatalogEntry {
                    connection_index: 0,
                    item_index: 0
                }
            })
        ));
        let first = page(
            vec![
                entry(0, "not-enabled", true),
                entry(0, "same-model", false),
                entry(0, "same-model", true),
            ],
            json!({"part":"connection","connectionIndex":1}),
        );
        catalog.complete(Ok(first.clone()));
        assert_eq!(
            catalog.rows.len(),
            1,
            "both enabled model IDs and chat capability are required"
        );
        assert!(catalog.selection().is_none());
        catalog.move_selection(true);
        let selected = catalog.selection().unwrap().choice.clone();
        assert_eq!(selected.connection_id, "id-0");
        catalog.change_page(true);
        catalog.query().unwrap();
        catalog.complete(Ok(page(
            vec![header(1, false), enabled(1), entry(1, "same-model", true)],
            json!({"part":"connection","connectionIndex":2}),
        )));
        assert!(!catalog.ready(), "disabled-only page is skipped");
        catalog.query().unwrap();
        catalog.complete(Ok(page(
            vec![header(2, true), enabled(2), entry(2, "same-model", true)],
            Value::Null,
        )));
        assert_eq!(catalog.rows.len(), 1);
        assert!(
            catalog.selected.is_none(),
            "equal display names/model IDs cannot bind another connection"
        );
        catalog.change_page(false);
        assert!(matches!(
            catalog.query(),
            Some(Query::Continue {
                cursor: Cursor::CatalogEntry {
                    connection_index: 0,
                    ..
                },
                ..
            })
        ));
        catalog.complete(Ok(first));
        catalog.selected = Some(selected.clone());
        assert_eq!(
            catalog.selection().unwrap().choice,
            selected,
            "back restores the original header basis"
        );
        catalog.refresh();
        assert!(
            catalog.selection().is_none(),
            "visible local intent is not a fresh submission"
        );
        catalog.query().unwrap();
        catalog.refresh();
        catalog.complete(Ok(page(vec![], Value::Null)));
        assert!(catalog.selection().is_none());
        assert_eq!(
            catalog.query(),
            Some(Query::Start),
            "in-flight invalidation restarts indices"
        );
        catalog.complete(Ok(page(
            vec![header(2, true), enabled(2), entry(2, "same-model", true)],
            Value::Null,
        )));
        assert!(catalog.selected.is_none());
        assert!(
            !catalog.can_previous(),
            "old revision back cursors are discarded"
        );
        catalog.refresh();
        catalog.query().unwrap();
        catalog.complete(Err("offline".into()));
        assert!(
            catalog.error && catalog.query().is_none(),
            "errors do not create a retry loop"
        );
    }
}
