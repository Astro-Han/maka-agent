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

//! Project catalog and bounded directory wire contracts.
//! Decode at the JSON boundary; the owned types do not replace wire validation.

mod decode;
mod operations;
mod output;
pub use decode::{decode_mutation, decode_query};
pub use operations::{MUTATION_ERRORS, QUERY_ERRORS, decode_input, decode_output, supports};
pub use output::{assert_query_output, decode_mutation_result, decode_query_result};
use serde::Serialize;

pub const PAGE_ITEMS: usize = 64;
pub const PAGE_BYTES: usize = 48 * 1024;
pub const DIRECTORY_PAGE_ITEMS: usize = 128;
pub const DIRECTORY_PAGE_BYTES: usize = 32 * 1024;
pub const DIRECTORY_MAX_ENTRIES: usize = 4096;
pub const DIRECTORY_MAX_ROOTS: usize = 8;
pub const DIRECTORY_MAX_SEGMENTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    Summary,
    Locations,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Query {
    ListStart {
        view: View,
    },
    ListContinue {
        view: View,
        revision: String,
        cursor: String,
    },
    DirectoryRoots,
    /// Resolve a published selection for path-bearing input; requires Host path authority.
    DirectoryResolve {
        root_id: String,
        segments: Vec<String>,
    },
    DirectoryListStart {
        root_id: String,
        segments: Vec<String>,
    },
    DirectoryListContinue {
        root_id: String,
        segments: Vec<String>,
        cursor: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Mutation {
    Register {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prefer: Option<bool>,
    },
    RegisterDirectory {
        root_id: String,
        segments: Vec<String>,
    },
    Relink {
        project_id: String,
        path: String,
    },
    Rename {
        project_id: String,
        name: String,
    },
    Archive {
        project_id: String,
    },
    Restore {
        project_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub aliases: Vec<String>,
    pub name: String,
    pub location_count: u64,
    pub archived_at: Option<u64>,
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub path: String,
    pub is_worktree: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PageItem {
    Project {
        project_index: u64,
        id: String,
        name: String,
        alias_count: u64,
        location_count: u64,
        preferred_location_index: Option<u64>,
        archived_at: Option<u64>,
        available: bool,
    },
    Alias {
        project_index: u64,
        item_index: u64,
        alias: String,
    },
    Location {
        project_index: u64,
        item_index: u64,
        location: Location,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DirectoryRoot {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum QueryResult {
    Page {
        view: View,
        revision: String,
        project_count: u64,
        items: Vec<PageItem>,
        next_cursor: Option<String>,
    },
    RevisionChanged {
        view: View,
        expected: String,
        actual: String,
    },
    DirectoryRoots {
        roots: Vec<DirectoryRoot>,
    },
    DirectoryPath {
        root_id: String,
        segments: Vec<String>,
        path: String,
    },
    DirectoryPage {
        root_id: String,
        segments: Vec<String>,
        entries: Vec<DirectoryEntry>,
        next_cursor: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MutationResult {
    Project { project: Project },
}

impl Query {
    pub fn uses_host_paths(&self) -> bool {
        matches!(
            self,
            Self::ListStart {
                view: View::Locations
            } | Self::ListContinue {
                view: View::Locations,
                ..
            } | Self::DirectoryResolve { .. }
        )
    }
}

impl Mutation {
    pub fn uses_host_paths(&self) -> bool {
        matches!(self, Self::Register { .. } | Self::Relink { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn directory_resolution_requires_path_authority_and_exact_selection() {
        let input = json!({"kind":"directory_resolve","rootId":"root-1","segments":["目录"]});
        let query = decode_query(&input).unwrap();
        assert!(query.uses_host_paths());
        assert!(!Query::DirectoryRoots.uses_host_paths());
        for path in ["/workspace/目录", "C:\\workspace\\目录"] {
            let output =
                json!({"kind":"directory_path","rootId":"root-1","segments":["目录"],"path":path});
            assert_query_output(&query, &decode_query_result(&output).unwrap()).unwrap();
            for (field, value) in [("rootId", json!("other")), ("segments", json!([]))] {
                let mut wrong = output.clone();
                wrong[field] = value;
                assert!(
                    assert_query_output(&query, &decode_query_result(&wrong).unwrap()).is_err()
                );
            }
            let mut relative = output;
            relative["path"] = json!("workspace/目录");
            assert!(decode_query_result(&relative).is_err());
        }
        for segment in ["..", ".", "a/b", "a\\b", ""] {
            let mut invalid = input.clone();
            invalid["segments"] = json!([segment]);
            assert!(decode_query(&invalid).is_err());
        }
    }
}
