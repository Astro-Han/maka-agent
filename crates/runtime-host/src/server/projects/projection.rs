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

use super::{Result, internal, invalid};
use maka_event_log::projects::ProjectRecord;
use maka_protocol::project::{self, Location, PageItem, Project, Query, QueryResult, View};
use sha2::{Digest, Sha256};

pub(super) async fn available_paths(record: ProjectRecord) -> Result<Vec<String>> {
    tokio::task::spawn_blocking(move || {
        available(&record)
            .into_iter()
            .map(|index| record.locations[index].path.clone())
            .collect()
    })
    .await
    .map_err(internal)
}

fn available(record: &ProjectRecord) -> Vec<usize> {
    let mut locations: Vec<_> = record
        .locations
        .iter()
        .enumerate()
        .filter(|(_, location)| std::path::Path::new(&location.path).is_dir())
        .map(|(index, _)| index)
        .collect();
    locations.sort_by(|&a, &b| {
        record.locations[b]
            .last_used_at
            .cmp(&record.locations[a].last_used_at)
            .then_with(|| record.locations[a].path.cmp(&record.locations[b].path))
    });
    locations
}

pub(super) async fn project(record: ProjectRecord) -> Result<Project> {
    tokio::task::spawn_blocking(move || Project {
        available: !available(&record).is_empty(),
        id: record.id,
        aliases: record.aliases,
        name: record.name,
        location_count: record.locations.len() as u64,
        archived_at: record.archived_at,
    })
    .await
    .map_err(internal)
}

pub(super) async fn query(records: Vec<ProjectRecord>, input: Query) -> Result<QueryResult> {
    tokio::task::spawn_blocking(move || {
        let view = match &input {
            Query::ListStart { view } | Query::ListContinue { view, .. } => *view,
            _ => return Err(invalid("not a project list query")),
        };
        let project_count = records.len() as u64;
        let mut items = Vec::new();
        for (index, record) in records.into_iter().enumerate() {
            let available = available(&record);
            let project_index = index as u64;
            items.push(PageItem::Project {
                project_index,
                id: record.id,
                name: record.name,
                alias_count: record.aliases.len() as u64,
                location_count: record.locations.len() as u64,
                preferred_location_index: available.first().map(|&index| index as u64),
                archived_at: record.archived_at,
                available: !available.is_empty(),
            });
            items.extend(
                record
                    .aliases
                    .into_iter()
                    .enumerate()
                    .map(|(index, alias)| PageItem::Alias {
                        project_index,
                        item_index: index as u64,
                        alias,
                    }),
            );
            if view == View::Locations {
                items.extend(
                    record
                        .locations
                        .into_iter()
                        .enumerate()
                        .map(|(index, location)| PageItem::Location {
                            project_index,
                            item_index: index as u64,
                            location: Location {
                                path: location.path,
                                is_worktree: location.is_worktree,
                            },
                        }),
                );
            }
        }
        let revision = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&items).map_err(internal)?)
        );
        let offset = match input {
            Query::ListContinue {
                revision: expected,
                cursor,
                ..
            } => {
                if expected != revision {
                    return Ok(QueryResult::RevisionChanged {
                        view,
                        expected,
                        actual: revision,
                    });
                }
                if cursor.is_empty()
                    || !cursor.bytes().all(|b| b.is_ascii_digit())
                    || (cursor.starts_with('0') && cursor.len() != 1)
                {
                    return Err(invalid("invalid project catalog cursor"));
                }
                let offset: usize = cursor.parse().map_err(invalid)?;
                if offset >= items.len() {
                    return Err(invalid("project catalog cursor is out of range"));
                }
                offset
            }
            _ => 0,
        };
        let mut page_items = Vec::new();
        let page = |items, next_cursor| QueryResult::Page {
            view,
            revision: revision.clone(),
            project_count,
            items,
            next_cursor,
        };
        for index in offset..items.len() {
            let mut candidate = page_items.clone();
            candidate.push(items[index].clone());
            if candidate.len() > project::PAGE_ITEMS
                || serde_json::to_vec(&page(
                    candidate.clone(),
                    (index + 1 < items.len()).then(|| (index + 1).to_string()),
                ))
                .map_err(internal)?
                .len()
                    > project::PAGE_BYTES
            {
                if page_items.is_empty() {
                    return Err(internal("project catalog item exceeds response limit"));
                }
                return Ok(page(page_items, Some(index.to_string())));
            }
            page_items = candidate;
        }
        Ok(page(page_items, None))
    })
    .await
    .map_err(internal)?
}
