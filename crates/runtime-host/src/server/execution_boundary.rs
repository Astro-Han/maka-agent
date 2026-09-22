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

use super::{Host, HostError, sessions};
use crate::session::SessionConfiguration;
use maka_protocol::session::SandboxMode;
use maka_protocol::{
    OperationError, OperationErrorCode as Code, Outcome, execution_boundary as wire,
};
use serde_json::Value;

/// The same persisted policy configures tool scopes at admission. No memory/UI fallback.
/// This projection describes authorization, not platform sandbox availability.
pub(super) async fn execute(host: &Host, value: &Value) -> Result<Outcome, HostError> {
    let session_id = wire::decode_input(value)?;
    let record = match host
        .log
        .get_session::<SessionConfiguration>(&session_id)
        .await
    {
        Ok(Some(record)) => record,
        Ok(None) => {
            return Ok(Outcome::failure(OperationError {
                code: Code::NotFound,
                message: "Session does not exist".into(),
            }));
        }
        Err(error) => return Ok(Outcome::failure(sessions::stored(error))),
    };
    let config = record.configuration;
    let revision = config.boundary_revision;
    let boundary = match config.sandbox_mode {
        SandboxMode::ReadOnly => wire::ExecutionBoundarySummary::Managed {
            access: wire::ManagedAccess::ReadOnly,
            revision,
        },
        SandboxMode::WorkspaceWrite => wire::ExecutionBoundarySummary::Managed {
            access: wire::ManagedAccess::Writable,
            revision,
        },
        SandboxMode::DangerFullAccess => {
            wire::ExecutionBoundarySummary::DangerFullAccess { revision }
        }
    };
    let output = serde_json::to_value(boundary)?;
    wire::decode_output(&output)?;
    Ok(Outcome::success(output))
}

pub(super) const ERRORS: &[Code] = &[
    Code::HostNotReady,
    Code::HostDraining,
    Code::OperationUnavailable,
    Code::InvalidRequest,
    Code::PersistenceFailed,
    Code::InternalFailure,
    Code::NotFound,
];

#[cfg(test)]
mod tests {
    use super::*;
    use maka_event_log::root::{RootNamespaces, RootOwner};
    use maka_runtime::{
        event::{EventWrite, Fact, Invocation, InvocationInput, RuntimeEvent},
        execution::{
            ApprovalPolicy, BehaviorId, CollaborationMode, WorkspaceProjection, WorkspaceTarget,
        },
        tool_call::{ToolCallIdentity, ToolOrigin},
        tools::{ToolCallContext, ToolJournal},
    };
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn prepared_native_tools_cannot_cross_a_permission_revision_and_next_call_refreshes_policy()
     {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let cwd = maka_fs_tools::workspace::project::host_path(&workspace)
            .unwrap()
            .to_owned();
        let namespaces = RootNamespaces {
            ownership: temp.path().join("owners"),
            control: temp.path().join("control"),
        };
        let host = Host::open(RootOwner::create(&temp.path().join("root"), &namespaces).unwrap())
            .await
            .unwrap();
        let configuration = SessionConfiguration {
            workspace_origin: maka_runtime::execution::WorkspaceOrigin::Selected,
            workspace: WorkspaceProjection {
                target: WorkspaceTarget::HostPath { path: cwd.clone() },
                host_cwd: cwd.clone(),
            },
            worktree: None,
            name: "Permission race".into(),
            labels: Vec::new(),
            is_flagged: false,
            title_is_manual: false,
            target: crate::session::SessionTarget::Executor {
                executor_id: "fixture".to_owned().try_into().unwrap(),
            },
            connection_locked: false,
            thinking_level: None,
            tool_profile: None,
            bound_tools: None,
            instructions: None,
            sandbox_mode: SandboxMode::DangerFullAccess,
            approval_policy: ApprovalPolicy::Never,
            boundary_revision: 0,
            collaboration_mode: CollaborationMode::Agent,
            orchestration_mode: BehaviorId::default(),
        };
        host.log
            .create_session("race", "create", &configuration, 1)
            .await
            .unwrap();
        let invocation = Invocation {
            session_id: "race".into(),
            turn_id: "turn".into(),
            run_id: "run".into(),
            invocation_id: "invocation".into(),
        };
        host.log
            .append(
                &EventWrite::plain(RuntimeEvent::new(
                    invocation.clone(),
                    Fact::InvocationOpened {
                        input: InvocationInput::Message {
                            content: "Run a command".into(),
                            source_messages: Vec::new(),
                            request_fingerprint: None,
                        },
                        configuration: Some(Box::new(
                            configuration.invocation_configuration().await.unwrap(),
                        )),
                    },
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        let catalog = host
            .executions
            .preview_tool_catalog(
                Some("race"),
                uuid::Uuid::new_v4(),
                &cwd,
                SandboxMode::DangerFullAccess,
                None,
            )
            .await
            .unwrap();
        let input = serde_json::json!({"command":"echo escaped > escaped.txt"});
        let context = ToolCallContext {
            invocation: invocation.clone(),
            operation_id: "shell".into(),
        };
        let mut prepared = Vec::new();
        for (name, input, boundary) in [
            (maka_process::SHELL_NAME, input.clone(), "command"),
            (
                maka_fs_tools::WRITE_NAME,
                serde_json::json!({"path":"escaped.txt","content":"escaped"}),
                "file",
            ),
            (
                maka_fs_tools::READ_NAME,
                serde_json::json!({"path":"escaped.txt"}),
                "file",
            ),
        ] {
            let context = ToolCallContext {
                invocation: invocation.clone(),
                operation_id: name.into(),
            };
            let effect = catalog
                .prepare(
                    name.into(),
                    input.clone(),
                    context.clone(),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            prepared.push((name, input, boundary, context, effect));
        }
        // Place the revision change exactly between preparation and native admission.
        let record = host
            .log
            .get_session::<SessionConfiguration>("race")
            .await
            .unwrap()
            .unwrap();
        host.log
            .update_session_metadata(
                "race",
                record.revision,
                |config: &mut SessionConfiguration| {
                    config.sandbox_mode = SandboxMode::ReadOnly;
                    config.boundary_revision += 1;
                    Ok(())
                },
            )
            .await
            .unwrap();
        for (name, input, boundary, context, effect) in prepared {
            let error = ToolJournal::new(host.log.clone(), invocation.clone())
                .invoke_prepared_call(
                    context.operation_id,
                    ToolCallIdentity {
                        tool_call_id: name.into(),
                        origin: ToolOrigin::Standalone,
                    },
                    name.into(),
                    input,
                    CancellationToken::new(),
                    effect,
                )
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("permissions changed before {boundary} admission")),
                "{name}: {error}"
            );
        }
        assert!(!workspace.join("escaped.txt").exists());
        assert!(
            host.log
                .query_shell_resources("race", None, 0)
                .await
                .unwrap()
                .resources
                .is_empty()
        );
        // No sleeps or scheduling assumptions: the same catalog must discard its old handler.
        drop(
            catalog
                .prepare(
                    maka_process::SHELL_NAME.into(),
                    input.clone(),
                    context.clone(),
                    CancellationToken::new(),
                )
                .await
                .unwrap(),
        );
        let record = host
            .log
            .get_session::<SessionConfiguration>("race")
            .await
            .unwrap()
            .unwrap();
        host.log
            .update_session_metadata(
                "race",
                record.revision,
                |config: &mut SessionConfiguration| {
                    config.approval_policy = ApprovalPolicy::OnRequest;
                    config.boundary_revision += 1;
                    Ok(())
                },
            )
            .await
            .unwrap();
        drop(
            catalog
                .prepare(
                    maka_process::SHELL_NAME.into(),
                    input,
                    context,
                    CancellationToken::new(),
                )
                .await
                .unwrap(),
        );
        host.draining.cancel();
        host.plugin_tasks.close();
        host.plugin_tasks.wait().await;
        host.executions.shutdown().await;
        host.shells.shutdown().await;
        drop(catalog);
        host.log.shutdown().await.unwrap();
        host.configuration.shutdown().await.unwrap();
    }
}
