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

use super::{decode::*, *};
use crate::Result;
use serde_json::Value;
use std::collections::HashSet;

pub fn decode_query_result(value: &Value) -> Result<QueryResult> {
    let result = match value["kind"].as_str() {
        Some("page") => {
            exact(
                value,
                &[
                    "kind",
                    "view",
                    "revision",
                    "projectCount",
                    "items",
                    "nextCursor",
                ],
            )?;
            let view = view(&value["view"])?;
            QueryResult::Page {
                view,
                revision: revision(&value["revision"])?,
                project_count: count(&value["projectCount"])?,
                items: array(&value["items"], PAGE_ITEMS)?
                    .iter()
                    .map(|v| page_item(v, view))
                    .collect::<Result<_>>()?,
                next_cursor: nullable(&value["nextCursor"], |v| text(v, 128))?,
            }
        }
        Some("revision_changed") => {
            exact(value, &["kind", "view", "expected", "actual"])?;
            QueryResult::RevisionChanged {
                view: view(&value["view"])?,
                expected: revision(&value["expected"])?,
                actual: revision(&value["actual"])?,
            }
        }
        Some("directory_roots") => {
            exact(value, &["kind", "roots"])?;
            let roots = array(&value["roots"], DIRECTORY_MAX_ROOTS)?.iter().map(|root| {
                exact(root, &["id", "label"])?;
                let label = text(&root["label"], 128)?;
                let whitespace = |c: char| matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}');
                if label.trim_matches(whitespace) != label || label.chars().any(|c| c <= '\u{001f}' || c == '\u{007f}') {
                    return Err(invalid());
                }
                Ok(DirectoryRoot { id: id(&root["id"])?, label })
            }).collect::<Result<_>>()?;
            QueryResult::DirectoryRoots { roots }
        }
        Some("directory_page") => {
            exact(
                value,
                &["kind", "rootId", "segments", "entries", "nextCursor"],
            )?;
            QueryResult::DirectoryPage {
                root_id: id(&value["rootId"])?,
                segments: segments(&value["segments"])?,
                entries: array(&value["entries"], DIRECTORY_PAGE_ITEMS)?
                    .iter()
                    .map(|entry| {
                        exact(entry, &["name"])?;
                        Ok(DirectoryEntry {
                            name: segment(&entry["name"])?,
                        })
                    })
                    .collect::<Result<_>>()?,
                next_cursor: nullable(&value["nextCursor"], segment)?,
            }
        }
        _ => return Err(invalid()),
    };
    let limit = match &result {
        QueryResult::Page { .. } => Some(PAGE_BYTES),
        QueryResult::DirectoryPage { .. } => Some(DIRECTORY_PAGE_BYTES),
        _ => None,
    };
    if let Some(limit) = limit
        && serde_json::to_vec(&result).map_err(|_| invalid())?.len() > limit
    {
        return Err(invalid());
    }
    Ok(result)
}

pub fn decode_mutation_result(value: &Value) -> Result<MutationResult> {
    exact(value, &["kind", "project"])?;
    if value["kind"] != "project" {
        return Err(invalid());
    }
    let value = &value["project"];
    exact(
        value,
        &[
            "id",
            "aliases",
            "name",
            "locationCount",
            "archivedAt",
            "available",
        ],
    )?;
    let aliases = array(&value["aliases"], usize::MAX)?
        .iter()
        .map(id)
        .collect::<Result<Vec<_>>>()?;
    if aliases.iter().collect::<HashSet<_>>().len() != aliases.len() {
        return Err(invalid());
    }
    Ok(MutationResult::Project {
        project: Project {
            id: id(&value["id"])?,
            aliases,
            name: text(&value["name"], 16 * 1024)?,
            location_count: count(&value["locationCount"])?,
            archived_at: nullable(&value["archivedAt"], count)?,
            available: boolean(&value["available"])?,
        },
    })
}

pub fn assert_query_output(input: &Query, output: &QueryResult) -> Result<()> {
    match input {
        Query::ListStart { view } | Query::ListContinue { view, .. } => {
            let actual = match output {
                QueryResult::Page { view, .. } | QueryResult::RevisionChanged { view, .. } => view,
                _ => return Err(invalid()),
            };
            if view != actual {
                return Err(invalid());
            }
        }
        Query::DirectoryListStart { root_id, .. }
        | Query::DirectoryListContinue { root_id, .. } => {
            if !matches!(output, QueryResult::DirectoryPage { root_id: actual, .. } if actual == root_id)
            {
                return Err(invalid());
            }
        }
        Query::DirectoryRoots => {}
    }
    Ok(())
}

fn page_item(value: &Value, view: View) -> Result<PageItem> {
    Ok(match value["kind"].as_str() {
        Some("project") => {
            exact(
                value,
                &[
                    "kind",
                    "projectIndex",
                    "id",
                    "name",
                    "aliasCount",
                    "locationCount",
                    "preferredLocationIndex",
                    "archivedAt",
                    "available",
                ],
            )?;
            PageItem::Project {
                project_index: count(&value["projectIndex"])?,
                id: id(&value["id"])?,
                name: text(&value["name"], 16 * 1024)?,
                alias_count: count(&value["aliasCount"])?,
                location_count: count(&value["locationCount"])?,
                preferred_location_index: nullable(&value["preferredLocationIndex"], count)?,
                archived_at: nullable(&value["archivedAt"], count)?,
                available: boolean(&value["available"])?,
            }
        }
        Some("alias") => {
            exact(value, &["kind", "projectIndex", "itemIndex", "alias"])?;
            PageItem::Alias {
                project_index: count(&value["projectIndex"])?,
                item_index: count(&value["itemIndex"])?,
                alias: id(&value["alias"])?,
            }
        }
        Some("location") if view == View::Locations => {
            exact(value, &["kind", "projectIndex", "itemIndex", "location"])?;
            exact(&value["location"], &["path", "isWorktree"])?;
            PageItem::Location {
                project_index: count(&value["projectIndex"])?,
                item_index: count(&value["itemIndex"])?,
                location: Location {
                    path: path(&value["location"]["path"])?,
                    is_worktree: boolean(&value["location"]["isWorktree"])?,
                },
            }
        }
        _ => return Err(invalid()),
    })
}
