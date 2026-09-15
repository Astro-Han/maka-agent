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
use maka_fs_tools::workspace::directory::PublishedDirectory;
use maka_protocol::project::{self, DirectoryEntry, DirectoryRoot, Query, QueryResult};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryRootSpec {
    pub label: String,
    pub path: PathBuf,
}

impl DirectoryRootSpec {
    /// Validate the published directory set and capture canonical paths once.
    pub fn normalize(specs: Vec<Self>) -> Result<Vec<Self>> {
        Ok(Directories::open(Some(specs))?
            .0
            .into_iter()
            .map(|root| Self {
                label: root.label,
                path: root.directory.path().to_owned(),
            })
            .collect())
    }
}

pub(in crate::server) struct Directories(Vec<Root>);

struct Root {
    label: String,
    directory: Arc<PublishedDirectory>,
}

impl Directories {
    pub(in crate::server) fn open(specs: Option<Vec<DirectoryRootSpec>>) -> Result<Self> {
        let Some(specs) = specs else {
            #[cfg(unix)]
            let home = std::env::var_os("HOME").map(PathBuf::from);
            #[cfg(windows)]
            let home = maka_event_log::root::windows::account_home().ok();
            return Ok(Self(
                home.and_then(|path| PublishedDirectory::open(&path).ok())
                    .map(|directory| Root {
                        label: "~".into(),
                        directory: Arc::new(directory),
                    })
                    .into_iter()
                    .collect(),
            ));
        };
        if specs.len() > project::DIRECTORY_MAX_ROOTS {
            return Err(invalid("too many project roots"));
        }
        let mut roots: Vec<Root> = Vec::new();
        for spec in specs {
            let label = spec
                .label
                .trim_matches(|c| {
                    matches!(c,
                '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' |
                '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
                '\u{205f}' | '\u{3000}' | '\u{feff}')
                })
                .to_owned();
            let directory = Arc::new(PublishedDirectory::open(&spec.path).map_err(invalid)?);
            if roots
                .iter()
                .any(|r| r.label == label || r.directory.path() == directory.path())
            {
                return Err(invalid("duplicate project root"));
            }
            roots.push(Root { label, directory });
        }
        let result = Self(roots);
        project::decode_query_result(&serde_json::to_value(result.roots()).map_err(internal)?)
            .map_err(invalid)?;
        Ok(result)
    }

    fn roots(&self) -> QueryResult {
        QueryResult::DirectoryRoots {
            roots: self
                .0
                .iter()
                .enumerate()
                .map(|(index, root)| DirectoryRoot {
                    id: format!("root-{}", index + 1),
                    label: root.label.clone(),
                })
                .collect(),
        }
    }

    fn root(&self, id: &str) -> Result<Arc<PublishedDirectory>> {
        self.0
            .iter()
            .enumerate()
            .find(|(index, _)| id == format!("root-{}", index + 1))
            .map(|(_, root)| root.directory.clone())
            .ok_or_else(|| invalid("unknown project root"))
    }

    pub(super) async fn resolve(
        &self,
        id: &str,
        segments: Vec<String>,
    ) -> Result<(Arc<PublishedDirectory>, PathBuf)> {
        let directory = self.root(id)?;
        let work = directory.clone();
        let path = tokio::task::spawn_blocking(move || work.resolve(&segments))
            .await
            .map_err(internal)?
            .map_err(invalid)?;
        Ok((directory, path))
    }

    pub(super) async fn query(&self, input: Query) -> Result<QueryResult> {
        let (root_id, segments, cursor) = match input {
            Query::DirectoryRoots => return Ok(self.roots()),
            Query::DirectoryListStart { root_id, segments } => (root_id, segments, None),
            Query::DirectoryListContinue {
                root_id,
                segments,
                cursor,
            } => (root_id, segments, Some(cursor)),
            _ => return Err(invalid("not a directory query")),
        };
        let directory = self.root(&root_id)?;
        let selected = segments.clone();
        let names = tokio::task::spawn_blocking(move || {
            directory.directory_names(&selected, project::DIRECTORY_MAX_ENTRIES)
        })
        .await
        .map_err(internal)?
        .map_err(invalid)?;
        let names: Vec<_> = names
            .into_iter()
            .filter(|name| {
                cursor
                    .as_ref()
                    .is_none_or(|cursor| name.encode_utf16().cmp(cursor.encode_utf16()).is_gt())
            })
            .collect();
        let page = |entries, next_cursor| QueryResult::DirectoryPage {
            root_id: root_id.clone(),
            segments: segments.clone(),
            entries,
            next_cursor,
        };
        let mut entries = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let next = (index + 1 < names.len()).then(|| name.clone());
            let mut candidate = entries.clone();
            candidate.push(DirectoryEntry { name: name.clone() });
            if candidate.len() > project::DIRECTORY_PAGE_ITEMS
                || serde_json::to_vec(&page(candidate.clone(), next))
                    .map_err(internal)?
                    .len()
                    > project::DIRECTORY_PAGE_BYTES
            {
                let cursor = entries
                    .last()
                    .map(|entry: &DirectoryEntry| entry.name.clone())
                    .ok_or_else(|| invalid("project directory entry exceeds response limit"))?;
                return Ok(page(entries, Some(cursor)));
            }
            entries = candidate;
        }
        Ok(page(entries, None))
    }
}

/// Registration validates containment again after project discovery, before SQL.
pub(super) async fn validate_registration(
    root: Arc<PublishedDirectory>,
    path: &Path,
) -> Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || root.validate(&path))
        .await
        .map_err(internal)?
        .map_err(invalid)
}
