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

use super::sources::bounded;
use maka_config::skills::SkillPreferences;
use maka_protocol::skills::*;
use maka_runtime::artifact::content_digest;
use maka_skills::{DiscoveryFailure, OriginFailure, OriginStatus, SkillLocation, SourceCatalog};
use std::collections::BTreeMap;

/// No Run context is attached to this query: eligibility is not advertisement.
/// Mutations are not installed yet, so no row claims to be manageable.
pub(super) fn items(
    catalog: &SourceCatalog,
    preferences: Option<&SkillPreferences>,
) -> Vec<CatalogItem> {
    let snapshot = &catalog.publication;
    let managed: BTreeMap<_, _> = catalog
        .managed
        .inventory
        .iter()
        .map(|s| (s.location.id.as_str(), s.content_sha256.as_str()))
        .chain(
            catalog
                .managed
                .rejected
                .iter()
                .map(|s| (s.location.id.as_str(), s.content_sha256.as_str())),
        )
        .collect();
    let mut items = Vec::new();
    for skill in &snapshot.discovery.inventory {
        let mut item = row(&skill.location, preferences);
        metadata(
            &mut item,
            &skill.document.manifest.name,
            &skill.document.manifest.description,
            &skill.document.manifest.attributes.allowed_tools,
        );
        item.shadowed_by = skill.shadowed_by.clone().filter(|r| safe(r, 512));
        item.context_status = if skill.shadowed_by.is_some() {
            ContextStatus::Shadowed
        } else if item.runtime_status == SkillRuntimeStatus::StateError {
            ContextStatus::Invalid
        } else if !item.enabled {
            ContextStatus::Disabled
        } else {
            ContextStatus::Unknown
        };
        match snapshot
            .origins
            .get(&skill.location.reference)
            .map(|o| &o.status)
        {
            None | Some(OriginStatus::Missing) => {
                item.validation_status = ValidationStatus::MissingLock;
                item.validation_codes.push(SkillValidationCode::MissingLock);
            }
            Some(OriginStatus::Invalid(reason)) => {
                item.source_type = GovernanceSourceType::Unknown;
                item.validation_status = ValidationStatus::MetadataError;
                item.validation_codes.push(origin_code(*reason));
                item.managed_update_status = Some(ManagedUpdateStatus::MetadataError);
            }
            Some(
                origin @ (OriginStatus::Bundled { content_sha256 }
                | OriginStatus::Managed { content_sha256, .. }),
            ) => {
                item.user_modified = content_sha256 != &skill.content_sha256;
                if item.user_modified {
                    item.validation_status = ValidationStatus::Modified;
                    item.validation_codes.push(SkillValidationCode::Modified);
                }
                match origin {
                    OriginStatus::Bundled { .. } => {
                        item.source_type = GovernanceSourceType::Bundled
                    }
                    OriginStatus::Managed { source_id, .. } => {
                        item.source_type = GovernanceSourceType::Managed;
                        let source_hash = managed.get(source_id.as_str()).copied();
                        item.managed_update_status = Some(if item.user_modified {
                            ManagedUpdateStatus::LocalModified
                        } else {
                            match source_hash {
                                None => ManagedUpdateStatus::SourceMissing,
                                Some(hash) if hash == content_sha256 => {
                                    ManagedUpdateStatus::UpToDate
                                }
                                Some(_) => ManagedUpdateStatus::UpdateAvailable,
                            }
                        });
                    }
                    _ => unreachable!("validated installation origin"),
                }
            }
        }
        issues(&mut item, &skill.document.issues);
        items.push(project(item));
    }
    for skill in &snapshot.discovery.rejected {
        let mut item = row(&skill.location, preferences);
        let manifest = &skill.document.manifest;
        metadata(
            &mut item,
            manifest.name.as_deref().unwrap_or(""),
            manifest.description.as_deref().unwrap_or(""),
            &manifest.attributes.allowed_tools,
        );
        invalidate(&mut item);
        item.source_type = GovernanceSourceType::Unknown;
        issues(&mut item, &skill.document.issues);
        items.push(project(item));
    }
    for location in &snapshot.empty {
        let mut item = row(location, preferences);
        invalidate(&mut item);
        item.validation_codes
            .push(SkillValidationCode::MissingFrontmatter);
        items.push(project(item));
    }
    for diagnostic in &snapshot.discovery.diagnostics {
        // The digest preserves identity without exposing an unsafe filesystem path.
        let identity = content_digest(
            format!(
                "{:?}:{:?}:{}:{}",
                diagnostic.scope,
                diagnostic.source,
                diagnostic.precedence,
                diagnostic.path.display()
            )
            .as_bytes(),
        );
        let location = SkillLocation {
            reference: format!("discovery:{identity}"),
            id: format!("source-{}", diagnostic.precedence),
            path: diagnostic.path.clone(),
            discovery_root: diagnostic.path.clone(),
            scope: diagnostic.scope.clone(),
            source: diagnostic.source.clone(),
            precedence: diagnostic.precedence,
        };
        let mut item = row(&location, preferences);
        invalidate(&mut item);
        item.source_type = GovernanceSourceType::Unknown;
        item.managed_update_status = None;
        item.validation_codes.push(match diagnostic.reason {
            DiscoveryFailure::BlockedPath => SkillValidationCode::BlockedPath,
            DiscoveryFailure::ReadFailed => SkillValidationCode::ReadFailed,
            DiscoveryFailure::SourceTooLarge => SkillValidationCode::BodyTooLarge,
        });
        items.push(CatalogItem::DiscoveryDiagnostic(item));
    }
    items
}

fn row(location: &SkillLocation, preferences: Option<&SkillPreferences>) -> GovernanceItem {
    let preference = preferences
        .and_then(|p| p.entries.get(&location.reference))
        .copied()
        .unwrap_or_default();
    GovernanceItem {
        reference: location.reference.clone(),
        id: location.id.clone(),
        name: String::new(),
        description: String::new(),
        declared_tools: Vec::new(),
        metadata_truncated: false,
        source_type: GovernanceSourceType::Workspace,
        user_modified: false,
        validation_status: ValidationStatus::Ok,
        validation_codes: Vec::new(),
        managed_update_status: Some(ManagedUpdateStatus::NotManaged),
        enabled: preferences.is_some() && preference.enabled,
        pinned: preferences.is_some() && preference.pinned,
        runtime_status: if preferences.is_none() {
            SkillRuntimeStatus::StateError
        } else if preference.enabled {
            SkillRuntimeStatus::Enabled
        } else {
            SkillRuntimeStatus::Disabled
        },
        scope: location.scope.clone(),
        source: location.source.clone(),
        context_status: ContextStatus::Unknown,
        context_rank: None,
        shadowed_by: None,
        needs_review: false,
        manageable: false,
    }
}

fn metadata(item: &mut GovernanceItem, name: &str, description: &str, tools: &[String]) {
    let (name, tn) = bounded(name, 256);
    let (description, td) = bounded(description, 4096);
    let declared_tools: Vec<_> = tools
        .iter()
        .filter(|s| !s.is_empty())
        .take(16)
        .map(|s| bounded(s, 128).0)
        .collect();
    let truncated = tn || td || tools != declared_tools;
    item.name = name;
    item.description = description;
    item.declared_tools = declared_tools;
    item.metadata_truncated = truncated;
    if truncated {
        item.validation_codes
            .push(SkillValidationCode::ProjectionTruncated);
    }
}

fn invalidate(item: &mut GovernanceItem) {
    item.enabled = false;
    item.pinned = false;
    if item.runtime_status != SkillRuntimeStatus::StateError {
        item.runtime_status = SkillRuntimeStatus::Disabled;
    }
    item.validation_status = ValidationStatus::MetadataError;
    item.managed_update_status = Some(ManagedUpdateStatus::MetadataError);
    item.context_status = ContextStatus::Invalid;
}

fn issues(item: &mut GovernanceItem, issues: &[maka_skills::Issue]) {
    for issue in issues {
        if !item.validation_codes.contains(&issue.code) {
            item.validation_codes.push(issue.code);
        }
    }
}

fn project(mut item: GovernanceItem) -> CatalogItem {
    if safe(&item.reference, 512) && safe(&item.id, 256) {
        return CatalogItem::Skill(item);
    }
    item.reference = format!("discovery:{}", content_digest(item.reference.as_bytes()));
    item.id = "invalid-skill-id".into();
    invalidate(&mut item);
    item.source_type = GovernanceSourceType::Unknown;
    item.shadowed_by = None;
    item.validation_codes.push(SkillValidationCode::BlockedPath);
    CatalogItem::DiscoveryDiagnostic(item)
}

fn safe(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && !value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}

fn origin_code(reason: OriginFailure) -> SkillValidationCode {
    match reason {
        OriginFailure::InvalidJson => SkillValidationCode::InvalidJson,
        OriginFailure::UnsupportedSchema => SkillValidationCode::UnsupportedSchema,
        OriginFailure::IdMismatch => SkillValidationCode::IdMismatch,
        OriginFailure::InvalidHash => SkillValidationCode::InvalidHash,
        OriginFailure::UnsafePath => SkillValidationCode::LockSymlink,
        OriginFailure::ReadFailed | OriginFailure::TooLarge => SkillValidationCode::ReadFailed,
    }
}
