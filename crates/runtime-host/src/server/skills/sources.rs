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

use super::{Host, HostError, failure, page};
use maka_protocol::{OperationError, OperationErrorCode as Code, Outcome, skills::*};
use serde::Serialize;
use serde_json::Value;

pub(in crate::server) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::InternalFailure,
];

pub(in crate::server) async fn execute(host: &Host, value: &Value) -> Result<Outcome, HostError> {
    let input = decode_catalog_input(value)?;
    if host.draining.is_cancelled() {
        return Ok(Outcome::failure(failure(
            Code::HostDraining,
            "Host is draining",
        )));
    }
    let result = query(host, &input).await;
    match result {
        Err(error) => Ok(Outcome::failure(error)),
        Ok(result) => {
            let value = serde_json::to_value(result)?;
            decode_catalog_output(&value)?;
            Ok(Outcome::success(value))
        }
    }
}
async fn query(host: &Host, input: &CatalogInput) -> Result<CatalogResult, OperationError> {
    let workspace = super::super::sessions::workspace::resolve(host, &input.context().workspace)
        .await
        .map_err(|mut e| {
            if matches!(e.code, Code::OperationConflict | Code::NotFound) {
                e.code = Code::OperationUnavailable;
            }
            e
        })?;
    let (sources, preferences) = if input.view() == CatalogView::Governance {
        host.executions
            .skill_governance(&workspace.host_cwd)
            .await?
    } else {
        (host.executions.skill_sources().await?, None)
    };
    let governance = super::governance::items(&sources, preferences.as_ref());
    let revision = maka_runtime::artifact::content_digest(&encode(&(
        "skill.catalog.v2",
        input.context(),
        input.view(),
        &workspace,
        &sources.publication.occupied,
        &sources.publication.origins,
        &governance,
        preferences.as_ref().map(|p| p.revision),
        sources
            .publication
            .discovery
            .inventory
            .iter()
            .map(|s| (&s.location.reference, &s.content_sha256))
            .collect::<Vec<_>>(),
        sources
            .publication
            .discovery
            .rejected
            .iter()
            .map(|s| (&s.location.reference, &s.content_sha256))
            .collect::<Vec<_>>(),
        sources
            .bundled
            .iter()
            .map(|s| (s.id, &s.content_sha256))
            .collect::<Vec<_>>(),
        sources
            .managed
            .inventory
            .iter()
            .map(|s| (&s.location.id, &s.content_sha256))
            .collect::<Vec<_>>(),
        sources
            .managed
            .rejected
            .iter()
            .map(|s| (&s.location.id, &s.content_sha256))
            .collect::<Vec<_>>(),
    ))?);
    let offset = match input {
        CatalogInput::Start { .. } => 0,
        CatalogInput::Continue {
            revision: expected,
            cursor,
            ..
        } => {
            if expected != &revision {
                return Ok(CatalogResult::RevisionChanged {
                    expected_revision: expected.clone(),
                    actual_revision: revision,
                    resolved_workspace: workspace,
                });
            }
            page::decode_cursor(cursor, &revision)?
        }
    };
    let mut items = match input.view() {
        CatalogView::Bundled => sources
            .bundled
            .iter()
            .map(|s| {
                let fields = &s.document.manifest;
                let (name, tn) = bounded(&fields.name, 256);
                let (description, td) = bounded(&fields.description, 4096);
                let (category, tc) = bounded(
                    fields
                        .attributes
                        .category
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("效率工具"),
                    128,
                );
                let tools = &fields.attributes.allowed_tools;
                let declared_tools: Vec<_> =
                    tools.iter().take(16).map(|s| bounded(s, 128).0).collect();
                let tt = tools.len() != declared_tools.len()
                    || tools.iter().zip(&declared_tools).any(|(a, b)| a != b);
                CatalogItem::Bundled {
                    id: s.id.into(),
                    name,
                    description,
                    category,
                    declared_tools,
                    metadata_truncated: tn || td || tc || tt,
                    installed: sources.publication.occupied.contains(&s.id.to_lowercase()),
                }
            })
            .collect::<Vec<_>>(),
        CatalogView::ManagedSources => {
            let installed = sources.installed_managed_sources();
            let valid = sources.managed.inventory.iter().map(|s| {
                (
                    &s.location.id,
                    Some(s.document.manifest.name.as_str()),
                    Some(s.document.manifest.description.as_str()),
                    s.document.manifest.attributes.category.as_deref(),
                )
            });
            let rejected = sources.managed.rejected.iter().map(|s| {
                (
                    &s.location.id,
                    s.document.manifest.name.as_deref(),
                    s.document.manifest.description.as_deref(),
                    s.document.manifest.attributes.category.as_deref(),
                )
            });
            valid
                .chain(rejected)
                .filter(|(id, ..)| maka_skills::safe_source_id(id))
                .map(|(id, name, description, category)| {
                    let (name, tn) = bounded(name.filter(|s| !s.is_empty()).unwrap_or(id), 256);
                    let (description, td) = bounded(description.unwrap_or(""), 4096);
                    let category = category
                        .filter(|value| {
                            matches!(
                                *value,
                                "内容创作"
                                    | "数据与AI"
                                    | "设计与UI"
                                    | "DevOps与部署"
                                    | "文档与写作"
                                    | "效率工具"
                                    | "研究与分析"
                            )
                        })
                        .unwrap_or("效率工具")
                        .to_owned();
                    CatalogItem::ManagedSource {
                        id: id.clone(),
                        name,
                        description,
                        category,
                        source_type: ManagedSourceType::Local,
                        metadata_truncated: tn || td,
                        installed: installed.contains(&id.to_ascii_lowercase()),
                    }
                })
                .collect()
        }
        CatalogView::Governance => governance,
    };
    items.sort_by(|a, b| key(a).cmp(&key(b)));
    if offset > items.len()
        || matches!(input, CatalogInput::Continue { .. }) && offset == items.len()
    {
        return Err(failure(Code::InvalidRequest, "Invalid skill source cursor"));
    }
    let view = input.view();
    let workspace_overhead = ",\"resolvedWorkspace\":".len() + encode(&workspace)?.len();
    let mut selected = Vec::new();
    let mut bytes = 0;
    for item in &items[offset..] {
        let end = offset + selected.len() + 1;
        let next_cursor = (end < items.len()).then(|| page::cursor(&revision, end));
        let envelope = CatalogResult::Page {
            view,
            revision: revision.clone(),
            items: Vec::new(),
            next_cursor,
            resolved_workspace: workspace.clone(),
        };
        let size = encode(item)?.len();
        if selected.len() == MAX_ITEMS
            || encode(&envelope)?.len() - workspace_overhead + bytes + size + selected.len()
                > MAX_PAGE_BYTES
        {
            if selected.is_empty() {
                return Err(failure(
                    Code::InternalFailure,
                    "Skill source metadata cannot fit a page",
                ));
            }
            break;
        }
        bytes += size;
        selected.push(item.clone());
    }
    let end = offset + selected.len();
    Ok(CatalogResult::Page {
        view,
        revision: revision.clone(),
        items: selected,
        next_cursor: (end < items.len()).then(|| page::cursor(&revision, end)),
        resolved_workspace: workspace,
    })
}
fn key(item: &CatalogItem) -> (&str, &str) {
    match item {
        CatalogItem::Bundled { name, id, .. } | CatalogItem::ManagedSource { name, id, .. } => {
            (name, id)
        }
        CatalogItem::Skill(item) | CatalogItem::DiscoveryDiagnostic(item) => {
            (&item.name, &item.reference)
        }
    }
}
pub(super) fn bounded(text: &str, max: usize) -> (String, bool) {
    let end = text.floor_char_boundary(text.len().min(max));
    (text[..end].into(), end < text.len())
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, OperationError> {
    serde_json::to_vec(value).map_err(|e| failure(Code::InternalFailure, &e.to_string()))
}
