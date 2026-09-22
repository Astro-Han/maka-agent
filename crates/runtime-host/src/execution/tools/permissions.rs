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

use super::live::NativeTools;
use maka_event_log::interactions::PermissionGrant;
use maka_runtime::{
    execution::SandboxMode, interaction::PermissionRequest, tool_call::ToolRejection,
    tools::ToolCallContext,
};
use maka_sandbox::{
    Network,
    filesystem::{Access, Rule},
    grant::Permissions,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

impl NativeTools {
    pub(super) async fn authorize_write(
        &self,
        name: &str,
        input: &Value,
        context: &ToolCallContext,
        boundary: (SandboxMode, u64),
        grants: &[PermissionGrant],
        cancellation: &CancellationToken,
    ) -> Result<Option<PermissionGrant>, ToolRejection> {
        let (mode, revision) = boundary;
        let origin = self.workspace_origin;
        let cwd = std::path::PathBuf::from(&self.cwd);
        let state_root = self.state_root.clone();
        let tool = name.to_owned();
        let input = input.clone();
        let additions: Vec<_> = grants
            .iter()
            .map(|grant| grant.permissions.clone())
            .collect();
        let (required, sandbox, ceiling) = tokio::task::spawn_blocking(move || {
            let paths = maka_fs_tools::mutation_paths(&tool, input, &cwd).map_err(invalid)?;
            let (mut sandbox, ceiling) =
                crate::execution::permissions::resolve(mode, &cwd, &state_root, origin)
                    .map_err(failed)?;
            for grant in additions {
                sandbox = sandbox.with_grant(&grant, &ceiling).map_err(failed)?;
            }
            let mut targets = std::collections::BTreeSet::new();
            for path in paths {
                let permission = crate::execution::permissions::materialize(Permissions {
                    filesystem: vec![Rule::exact(path, Access::Write)],
                    network: Network::Denied,
                })
                .map_err(invalid)?;
                if !sandbox.permits(&permission).map_err(failed)? {
                    targets.insert(permission.filesystem[0].path.clone());
                }
            }
            let required = Permissions {
                filesystem: targets
                    .into_iter()
                    .map(|path| Rule::exact(path, Access::Write))
                    .collect(),
                network: Network::Denied,
            };
            required.validate().map_err(invalid)?;
            if !ceiling.permits(&required).map_err(failed)? {
                return Err(denied(
                    "The requested mutation includes Host-protected files",
                ));
            }
            Ok((required, sandbox, ceiling))
        })
        .await
        .map_err(failed)??;
        if cancellation.is_cancelled() {
            return Err(ToolRejection::Cancelled);
        }
        if required.filesystem.is_empty() {
            return Ok(None);
        }
        let grant = self.interactions.request_permissions(
            &context.invocation,
            Some(&context.tool_use_id()),
            PermissionRequest {
                reason: format!("{name} needs write access to the listed files outside the current writable roots."),
                command: None,
                permissions: required.clone(),
            },
            revision,
            cancellation,
        ).await?;
        // A batch may contain several targets. Partial consent must not start
        // an only-partly-authorized mutation and discover the rest mid-patch.
        if !sandbox
            .with_grant(&grant.permissions, &ceiling)
            .map_err(failed)?
            .permits(&required)
            .map_err(failed)?
        {
            return Err(denied(
                "Not all target files were approved; no files were changed",
            ));
        }
        Ok(Some(grant))
    }
}

fn invalid(error: impl std::fmt::Display) -> ToolRejection {
    ToolRejection::InvalidInput {
        message: error.to_string(),
    }
}
fn failed(error: impl std::fmt::Display) -> ToolRejection {
    ToolRejection::PreparationFailed {
        message: error.to_string(),
    }
}
fn denied(message: &str) -> ToolRejection {
    ToolRejection::PolicyDenied {
        message: message.into(),
    }
}
