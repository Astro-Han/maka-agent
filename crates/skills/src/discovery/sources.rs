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

use super::{DiscoveryFailure, DiscoverySnapshot, OriginStatus, QuerySnapshot, ScanError, Source};
use crate::SkillDocument;
use maka_runtime::skills::{SkillScope, SkillSource};
use std::{collections::BTreeSet, path::Path};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct BundledSource {
    pub id: &'static str,
    pub document: SkillDocument,
    pub content_sha256: String,
}
#[derive(Debug)]
pub struct SourceCatalog {
    pub bundled: Vec<BundledSource>,
    pub managed: DiscoverySnapshot,
    pub publication: QuerySnapshot,
}
impl SourceCatalog {
    pub fn installed_managed_sources(&self) -> BTreeSet<String> {
        let mut installed = self.publication.occupied.clone();
        for skill in &self.publication.discovery.inventory {
            let location = &skill.location;
            if location.id.is_empty()
                || location.id.len() > 128
                || location.reference.len() > 384
                || location
                    .reference
                    .chars()
                    .any(|c| c <= '\u{1f}' || c == '\u{7f}')
            {
                continue;
            }
            if let Some(OriginStatus::Managed { source_id, .. }) = self
                .publication
                .origins
                .get(&location.reference)
                .map(|origin| &origin.status)
            {
                installed.insert(source_id.to_ascii_lowercase());
            }
        }
        installed
    }
}
#[derive(Debug, Clone, Copy)]
pub enum SourceCatalogError {
    Scan(ScanError),
    Read(DiscoveryFailure),
    InvalidBundledMetadata,
}
impl std::fmt::Display for SourceCatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Scan(error) => error.fmt(f),
            Self::Read(error) => write!(f, "skill source catalog is unavailable: {error:?}"),
            Self::InvalidBundledMetadata => f.write_str("bundled skill metadata is invalid"),
        }
    }
}
impl std::error::Error for SourceCatalogError {}

/// Package-owned metadata has one source of truth shared with the TS generator.
/// This private workspace crate is not published independently.
const COMPUTER_USE: &str =
    include_str!("../../../../packages/runtime/resources/bundled-skills/computer-use/SKILL.md");

pub(super) fn trusted_bundled_hash(id: &str, hash: &str) -> bool {
    // Historical hashes mirror the package generator's explicitly trusted releases.
    id == "computer-use"
        && (hash == maka_runtime::artifact::content_digest(COMPUTER_USE.as_bytes())
            || matches!(
                hash,
                "sha256:419088b2f8a0b12061b4811323abc381869ebe8fccbfc8f2bdfc96ff37a1e45b"
                    | "sha256:8e4404349be4e5493fcf13981624ed55198c0670a794fbf88e2bad81ddb79f6c"
                    | "sha256:64aa2ef2d608e15792cc04eff7204731671b6b18818964ba95c65f53c694db62"
            ))
}

pub fn source_catalog(
    root: &Path,
    home: Option<&Path>,
    cancellation: &CancellationToken,
) -> Result<SourceCatalog, SourceCatalogError> {
    let bundled = vec![BundledSource {
        id: "computer-use",
        document: crate::parse(COMPUTER_USE)
            .map_err(|_| SourceCatalogError::InvalidBundledMetadata)?,
        content_sha256: maka_runtime::artifact::content_digest(COMPUTER_USE.as_bytes()),
    }];
    let publication = Source::at(
        root,
        "skills",
        SkillScope::Workspace,
        SkillSource::Legacy,
        "workspace:legacy",
    );
    let publication =
        super::scan_with_origins(&[publication], cancellation).map_err(SourceCatalogError::Scan)?;
    if let Some(error) = publication
        .discovery
        .diagnostics
        .iter()
        .find(|d| d.path == root.join("skills"))
    {
        return Err(SourceCatalogError::Read(error.reason));
    }
    let managed = if let Some(home) = home {
        // The library is its own containment root; aliases may not reach other
        // home content. SKILL.md retains the ordinary nofollow/regular-file gate.
        let root = home.join(".maka/skill-sources");
        let source = Source::at(
            &root,
            "",
            SkillScope::Custom,
            SkillSource::Custom,
            "managed",
        );
        let snapshot = super::scan(&[source], cancellation).map_err(SourceCatalogError::Scan)?;
        if let Some(error) = snapshot.diagnostics.iter().find(|d| {
            d.path == root
                || matches!(
                    d.reason,
                    DiscoveryFailure::ReadFailed | DiscoveryFailure::SourceTooLarge
                )
        }) {
            return Err(SourceCatalogError::Read(error.reason));
        }
        snapshot
    } else {
        DiscoverySnapshot::default()
    };
    super::check_cancelled(cancellation).map_err(SourceCatalogError::Scan)?;
    Ok(SourceCatalog {
        bundled,
        managed,
        publication,
    })
}

pub fn safe_source_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 81
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
