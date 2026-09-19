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

use super::{Error, Skills, catalog};
use crate::{OriginStatus, api::*, discovery::artifact};
use maka_runtime::{artifact::content_digest, execution::WorkspaceProjection};

impl Skills {
    pub async fn preview_update(
        &self,
        input: &PreviewInput,
        workspace: WorkspaceProjection,
    ) -> Result<PreviewResult, Error> {
        let _call = self.basis.owner.admit().map_err(|_| Error::Retired)?;
        let _view = self.mutations.read().await;
        let (sources, preferences) = self.governance(&workspace.host_cwd).await?;
        let revision =
            catalog::revision(&input.context, &workspace, &sources, preferences.as_ref())?;
        let outcome = if revision != input.expected_revision {
            PreviewOutcome::RevisionConflict {
                expected_revision: input.expected_revision.clone(),
                actual_revision: revision,
            }
        } else {
            let reference = input.reference.clone();
            let cancellation = self.basis.owner.stopping().map_err(|_| Error::Retired)?;
            tokio::task::spawn_blocking(move || {
                preview(&sources, &reference, revision, &cancellation)
            })
            .await??
        };
        if !self.basis.owner.is_effective() {
            return Err(Error::Retired);
        }
        Ok(PreviewResult {
            outcome,
            resolved_workspace: workspace,
        })
    }
}

fn preview(
    sources: &crate::SourceCatalog,
    reference: &str,
    revision: String,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<PreviewOutcome, Error> {
    let reject = |reason| Ok(PreviewOutcome::Rejected { reason });
    let discovered = &sources.publication.discovery;
    let Some((location, current_hash)) = discovered
        .inventory
        .iter()
        .map(|skill| (&skill.location, &skill.content_sha256))
        .chain(
            discovered
                .rejected
                .iter()
                .map(|skill| (&skill.location, &skill.content_sha256)),
        )
        .find(|(location, _)| location.reference == reference)
    else {
        return reject(PreviewRejection::NotFound);
    };
    let origin = sources
        .publication
        .origins
        .get(reference)
        .map(|origin| &origin.status);
    let (source_id, baseline_hash) = match origin {
        Some(OriginStatus::Managed {
            source_id,
            content_sha256,
        }) => (source_id, content_sha256),
        Some(OriginStatus::Invalid(_)) => return reject(PreviewRejection::MetadataError),
        _ => return reject(PreviewRejection::NotManaged),
    };
    let Some(source) = sources
        .managed
        .inventory
        .iter()
        .find(|skill| &skill.location.id == source_id)
    else {
        return reject(
            if sources
                .managed
                .rejected
                .iter()
                .any(|skill| &skill.location.id == source_id)
            {
                PreviewRejection::SourceInvalid
            } else {
                PreviewRejection::SourceMissing
            },
        );
    };
    let current = artifact::read(location, true, cancellation)?;
    let source_bytes = artifact::read(&source.location, false, cancellation)?.content;
    let Some(current_bytes) = current.content else {
        return reject(PreviewRejection::MetadataError);
    };
    let Some(source_bytes) = source_bytes else {
        return reject(PreviewRejection::SourceMissing);
    };
    // Never pair a stale catalog revision with newly read, unconfirmed content.
    if content_digest(&current_bytes) != *current_hash
        || content_digest(&source_bytes) != source.content_sha256
    {
        return Err(Error::Source(
            "Skill content changed during preview; refresh the catalog".into(),
        ));
    }
    let Ok(current_text) = std::str::from_utf8(&current_bytes) else {
        return reject(PreviewRejection::MetadataError);
    };
    let Ok(source_text) = std::str::from_utf8(&source_bytes) else {
        return reject(PreviewRejection::SourceInvalid);
    };
    let current_lines = lines(current_text);
    let source_lines = lines(source_text);
    let summary = LineSummary {
        current_line_count: current_lines.len(),
        source_line_count: source_lines.len(),
        changed_line_count: (0..current_lines.len().max(source_lines.len()))
            .filter(|index| current_lines.get(*index) != source_lines.get(*index))
            .count(),
    };
    let (current_snippet, current_truncated) = snippet(current_text);
    let (source_snippet, source_truncated) = snippet(source_text);
    let mut outcome = PreviewOutcome::Preview {
        revision,
        current_snippet,
        source_snippet,
        current_truncated,
        source_truncated,
        has_managed_baseline: current
            .baseline
            .as_ref()
            .is_some_and(|bytes| content_digest(bytes) == *baseline_hash),
        summary,
        expected_current_sha256: current_hash.clone(),
        expected_source_sha256: source.content_sha256.clone(),
    };
    while serde_json::to_vec(&outcome)?.len() > MAX_PREVIEW_BYTES {
        let PreviewOutcome::Preview {
            current_snippet,
            source_snippet,
            current_truncated,
            source_truncated,
            ..
        } = &mut outcome
        else {
            unreachable!()
        };
        let (text, truncated) = if current_snippet.len() >= source_snippet.len() {
            (current_snippet, current_truncated)
        } else {
            (source_snippet, source_truncated)
        };
        text.truncate(text.floor_char_boundary(text.len().saturating_sub(256)));
        *truncated = true;
    }
    Ok(outcome)
}
fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}
fn snippet(text: &str) -> (String, bool) {
    let lines = lines(text);
    let text = lines
        .iter()
        .take(80)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let (snippet, truncated) = catalog::bounded(&text, 24 * 1024);
    (snippet, truncated || lines.len() > 80)
}
