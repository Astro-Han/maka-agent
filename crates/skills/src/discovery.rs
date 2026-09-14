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

use crate::{InvalidDocument, SkillDocument};
use maka_runtime::skills::{SkillScope, SkillSource};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

mod directory;
mod origin;
mod source;
mod sources;
pub use origin::{Origin, OriginFailure, OriginStatus};
pub use sources::{
    BundledSource, SourceCatalog, SourceCatalogError, governance_catalog, safe_source_id,
    source_catalog,
};
const MAX_ENTRIES: usize = 16_384;
const MAX_CONTENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Source {
    pub root: PathBuf,
    pub directory: PathBuf,
    pub scope: SkillScope,
    pub source: SkillSource,
    pub reference_prefix: String,
}

impl Source {
    /// The Host supplies all roots, including home; the library never searches ambient home.
    pub fn standard(cwd: &Path, workspace: &Path, home: Option<&Path>) -> Vec<Self> {
        let mut sources = vec![
            Self::at(
                cwd,
                ".maka/skills",
                SkillScope::Project,
                SkillSource::Maka,
                "project:maka",
            ),
            Self::at(
                cwd,
                ".agents/skills",
                SkillScope::Project,
                SkillSource::Agents,
                "project:agents",
            ),
            Self::at(
                workspace,
                "skills",
                SkillScope::Workspace,
                SkillSource::Legacy,
                "workspace:legacy",
            ),
        ];
        if let Some(home) = home {
            sources.extend([
                Self::at(
                    home,
                    ".maka/skills",
                    SkillScope::User,
                    SkillSource::Maka,
                    "user:maka",
                ),
                Self::at(
                    home,
                    ".agents/skills",
                    SkillScope::User,
                    SkillSource::Agents,
                    "user:agents",
                ),
            ]);
        }
        sources
    }

    fn at(
        root: &Path,
        directory: &str,
        scope: SkillScope,
        source: SkillSource,
        prefix: &str,
    ) -> Self {
        Self {
            root: root.into(),
            directory: directory.into(),
            scope,
            source,
            reference_prefix: prefix.into(),
        }
    }

    fn location(&self, id: String, precedence: usize) -> SkillLocation {
        SkillLocation {
            reference: format!("{}:{id}", self.reference_prefix),
            path: self.root.join(&self.directory).join(&id),
            discovery_root: self.root.clone(),
            id,
            scope: self.scope.clone(),
            source: self.source.clone(),
            precedence,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SkillLocation {
    pub reference: String,
    pub id: String,
    pub path: PathBuf,
    pub discovery_root: PathBuf,
    pub scope: SkillScope,
    pub source: SkillSource,
    pub precedence: usize,
}

#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    pub location: SkillLocation,
    pub document: SkillDocument,
    pub content_sha256: String,
    pub shadowed_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RejectedSkill {
    pub location: SkillLocation,
    pub document: InvalidDocument,
    pub content_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryFailure {
    BlockedPath,
    ReadFailed,
    SourceTooLarge,
}

#[derive(Debug, Clone)]
pub struct DiscoveryDiagnostic {
    pub path: PathBuf,
    pub scope: SkillScope,
    pub source: SkillSource,
    pub precedence: usize,
    pub reason: DiscoveryFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanError {
    Cancelled,
    LimitExceeded,
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "skill discovery cancelled",
            Self::LimitExceeded => "skill discovery exceeds inventory limits",
        })
    }
}
impl std::error::Error for ScanError {}

#[derive(Debug, Default)]
pub struct DiscoverySnapshot {
    pub inventory: Vec<DiscoveredSkill>,
    pub rejected: Vec<RejectedSkill>,
    pub diagnostics: Vec<DiscoveryDiagnostic>,
}

/// Query-only facts; ordinary runtime discovery does not inspect installation locks.
#[derive(Debug, Default)]
pub struct QuerySnapshot {
    pub discovery: DiscoverySnapshot,
    pub origins: BTreeMap<String, Origin>,
    pub occupied: BTreeSet<String>,
    pub empty: Vec<SkillLocation>,
}

fn scan_with_origins(
    sources: &[Source],
    cancellation: &CancellationToken,
) -> Result<QuerySnapshot, ScanError> {
    let mut query = QuerySnapshot::default();
    query.discovery = scan_impl(sources, cancellation, Some(&mut query))?;
    Ok(query)
}

/// Bounded blocking I/O; callers must join it before releasing execution ownership.
/// Preferences and capability gating are applied later, against this same snapshot.
pub fn scan(
    sources: &[Source],
    cancellation: &CancellationToken,
) -> Result<DiscoverySnapshot, ScanError> {
    scan_impl(sources, cancellation, None)
}

fn scan_impl(
    sources: &[Source],
    cancellation: &CancellationToken,
    mut query: Option<&mut QuerySnapshot>,
) -> Result<DiscoverySnapshot, ScanError> {
    if sources.len() > 32 {
        return Err(ScanError::LimitExceeded);
    }
    let mut snapshot = DiscoverySnapshot::default();
    let mut remaining_entries = MAX_ENTRIES;
    let mut remaining_bytes = MAX_CONTENT_BYTES;
    for (precedence, source) in sources.iter().enumerate() {
        check_cancelled(cancellation)?;
        let captured = match source::Captured::open(source) {
            Ok(Some(captured)) => captured,
            Ok(None) => continue,
            Err(reason) => {
                snapshot.diagnostic(
                    source,
                    precedence,
                    source.root.join(&source.directory),
                    reason,
                );
                continue;
            }
        };
        let entries = match captured.entries(&mut remaining_entries, cancellation) {
            Ok(entries) => entries,
            Err(source::EntriesError::Scan(error)) => return Err(error),
            Err(source::EntriesError::Read) => {
                snapshot.diagnostic(
                    source,
                    precedence,
                    source.root.join(&source.directory),
                    DiscoveryFailure::ReadFailed,
                );
                continue;
            }
        };
        let publication =
            source.scope == SkillScope::Workspace && source.source == SkillSource::Legacy;
        if publication && let Some(query) = query.as_deref_mut() {
            query.occupied.extend(
                entries
                    .iter()
                    .filter_map(|name| name.to_str().map(str::to_lowercase)),
            );
        }
        for name in entries {
            check_cancelled(cancellation)?;
            let path = source.root.join(&source.directory).join(&name);
            let Some(id) = name.to_str() else {
                snapshot.diagnostic(source, precedence, path, DiscoveryFailure::BlockedPath);
                continue;
            };
            let read =
                match captured.read_skill(&name, publication && query.is_some(), cancellation) {
                    Ok(Some(read)) => read,
                    Ok(None) => continue,
                    Err(source::ReadError::Cancelled) => return Err(ScanError::Cancelled),
                    Err(source::ReadError::Failure(reason)) => {
                        snapshot.diagnostic(source, precedence, path, reason);
                        continue;
                    }
                };
            let location = source.location(id.into(), precedence);
            let source::SkillRead::Document {
                bytes,
                origin,
                origin_bytes,
            } = read
            else {
                if publication && let Some(query) = query.as_deref_mut() {
                    query.empty.push(location);
                }
                continue;
            };
            remaining_bytes = remaining_bytes
                .checked_sub(bytes.len() + origin_bytes)
                .ok_or(ScanError::LimitExceeded)?;
            if let Some(origin) = origin
                && let Some(query) = query.as_deref_mut()
            {
                query.origins.insert(location.reference.clone(), origin);
            }
            match crate::parse(&String::from_utf8_lossy(&bytes)) {
                Ok(document) => snapshot.inventory.push(DiscoveredSkill {
                    location,
                    document,
                    content_sha256: maka_runtime::artifact::content_digest(&bytes),
                    shadowed_by: None,
                }),
                Err(document) => snapshot.rejected.push(RejectedSkill {
                    location,
                    document: *document,
                    content_sha256: maka_runtime::artifact::content_digest(&bytes),
                }),
            }
        }
    }
    check_cancelled(cancellation)?;
    snapshot.resolve_precedence();
    Ok(snapshot)
}

impl DiscoverySnapshot {
    fn diagnostic(
        &mut self,
        source: &Source,
        precedence: usize,
        path: PathBuf,
        reason: DiscoveryFailure,
    ) {
        self.diagnostics.push(DiscoveryDiagnostic {
            path,
            scope: source.scope.clone(),
            source: source.source.clone(),
            precedence,
            reason,
        });
    }
}

fn check_cancelled(token: &CancellationToken) -> Result<(), ScanError> {
    if token.is_cancelled() {
        Err(ScanError::Cancelled)
    } else {
        Ok(())
    }
}
