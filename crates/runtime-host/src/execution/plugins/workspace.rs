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

use super::{BoundCommands, Error, Executions, Receipt, SessionConfiguration, storage};
use maka_plugins::execution::{Progress, WorkspacePatch};
use maka_runtime::artifact::{Artifact, ArtifactKind, ArtifactSource};
use std::sync::Arc;

impl BoundCommands {
    pub(super) async fn export_workspace(
        &self,
        operation: String,
    ) -> Result<Option<WorkspacePatch>, Error> {
        let host = self.executions()?;
        let _gate = host.interactions.own_admission().await;
        let lease = self.context.admit().map_err(|_| Error::Revoked)?;
        let receipt = self.receipt(&host, &operation).await?;
        let (send, receive) = tokio::sync::oneshot::channel();
        let worker = host.clone();
        host.workers.spawn(async move {
            drop(_gate);
            let result = export(&worker, receipt).await;
            if matches!(result, Err(Error::OutcomeUnknown(_))) {
                worker.begin_drain();
            }
            drop(lease);
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| Error::OutcomeUnknown("workspace export owner disappeared".into()))?
    }
}

async fn export(host: &Arc<Executions>, receipt: Receipt) -> Result<Option<WorkspacePatch>, Error> {
    let session_id = &receipt.invocation.session_id;
    let turn_id = &receipt.invocation.turn_id;
    let id = maka_runtime::artifact::workspace_patch_id(session_id, turn_id);
    let gate = host.lock_admission().await;
    let session = host
        .log
        .get_session::<SessionConfiguration>(session_id)
        .await
        .map_err(storage)?
        .ok_or(Error::NotFound)?;
    let Some(binding) = session.configuration.worktree.clone() else {
        return Ok(None);
    };
    let descriptor = |record: Artifact| WorkspacePatch {
        artifact_id: record.id,
        session_id: record.session_id,
        turn_id: record.turn_id,
        bytes: record.size_bytes,
        base_commit: binding.base_commit().into(),
    };
    if let Some(record) = host
        .log
        .get_artifact(session_id, &id)
        .await
        .map_err(storage)?
        .record
    {
        if record.source != ArtifactSource::SubagentWriteback || record.turn_id != *turn_id {
            return Err(Error::Conflict);
        }
        return Ok(Some(descriptor(record)));
    }
    if !matches!(
        host.observe_plugin(receipt.clone()).await?.progress,
        Progress::Ended { .. }
    ) {
        return Err(Error::Busy);
    }
    if session
        .execution
        .as_ref()
        .is_none_or(|current| current.turn_id != *turn_id)
    {
        // This is not cleanup still in progress: the old filesystem snapshot
        // can no longer be reconstructed after a newer Turn was admitted.
        return Err(Error::Conflict);
    }
    let cwd = session.configuration.workspace.host_cwd;
    if binding.directory() != std::path::Path::new(&cwd) {
        return Err(Error::Conflict);
    }
    let fence = quiescent(host, &cwd).await?;
    drop(gate);
    let root = host.paths.state_root.join("subagent-worktrees");
    let capture = binding.clone();
    let bytes = super::super::workspaces::blocking(host.shutdown.clone(), move |cancel| {
        maka_fs_tools::worktree::Worktrees::open(&root)?.capture_patch(&capture, cancel)
    })
    .await
    .map_err(|e| Error::Host(e.to_string()))?;
    let _gate = host.lock_admission().await;
    if !host.accepting() {
        return Err(Error::Draining);
    }
    if quiescent(host, &cwd).await? != fence {
        return Err(Error::Busy);
    }
    let record = Artifact {
        id,
        session_id: session_id.clone(),
        turn_id: turn_id.clone(),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| Error::Host(e.to_string()))?
            .as_millis() as u64,
        name: "workspace.patch".into(),
        kind: ArtifactKind::Diff,
        size_bytes: bytes.len() as u64,
        mime_type: Some("text/x-diff".into()),
        source: ArtifactSource::SubagentWriteback,
        summary: Some(format!("Changes relative to {}", binding.base_commit())),
    };
    Ok(Some(descriptor(
        host.log
            .commit_artifact(record, bytes)
            .await
            .map_err(storage)?,
    )))
}

/// Check cleanup ownership as well as canonical execution. A finished Turn may
/// still own a process; a new Turn during capture invalidates the evidence fence.
async fn quiescent(
    host: &Executions,
    cwd: &str,
) -> Result<maka_event_log::workspace::WorkspaceFence, Error> {
    let fence = host.log.workspace_fence(cwd).await.map_err(storage)?;
    for member in &fence.0 {
        if member.orphaned_shells {
            return Err(Error::Host(format!(
                "cannot export workspace: Session {} has a shell with unknown cleanup after Host restart",
                member.session_id
            )));
        }
        let session = &member.session_id;
        if host.has_session_work(session).await.map_err(storage)?
            || host.plugin_processes.lock().unwrap().contains_key(session)
            || host
                .log
                .has_unsettled_shells(session)
                .await
                .map_err(storage)?
        {
            return Err(Error::Busy);
        }
    }
    Ok(fence)
}
