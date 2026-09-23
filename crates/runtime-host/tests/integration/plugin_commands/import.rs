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

use super::super::support::peer::Peer;
use maka_plugins::{
    execution::{CommandError, CreateRoot, RootApproval, RootTemplate},
    fiber::Fiber,
    session::import::{Command, Content, ImportState, Record, Source},
};
use maka_runtime_host::server::Host;
use serde_json::json;
use std::{path::Path, sync::Arc};

pub(super) async fn verify(
    host: &Arc<Host>,
    peer: &mut Peer,
    fiber: &Fiber,
    workspace: &Path,
    request: &CreateRoot,
    reopened: bool,
) {
    let source = host
        .authorize_plugin_execution(fiber.context(), &["scheduled-authority".into()])
        .await
        .unwrap()
        .boundaries()
        .unwrap()
        .remove(0);
    let maka_plugins::execution::Target::Model {
        model,
        thinking_level,
    } = &request.settings.target
    else {
        panic!("test requires a model");
    };
    let template = RootTemplate {
        workspace: maka_runtime::execution::WorkspaceTarget::HostPath {
            path: source.cwd.clone(),
        },
        cwd: source.cwd.clone(),
        workspace_identity: maka_fs_tools::workspace::ensure_identity(workspace)
            .await
            .unwrap(),
        model: model.clone(),
        thinking_level: *thinking_level,
        sandbox_mode: source.sandbox_mode,
        approval_policy: source.approval_policy,
        collaboration_mode: request.settings.collaboration_mode,
        orchestration_mode: request.settings.behavior.clone(),
    };
    let commands = host
        .authorize_plugin_root_execution(
            fiber.context(),
            RootApproval {
                template,
                source: Some(source),
            },
        )
        .unwrap();
    let operation_id = "import-current-ceilings".to_owned();
    let receipt = if !reopened {
        let receipt = commands
            .import_session(Command::Begin {
                root: Box::new(CreateRoot {
                    operation_id: operation_id.clone(),
                    ..request.clone()
                }),
                source: Source {
                    adapter: "foreign-format".into(),
                    session_id: "old-session".into(),
                },
            })
            .await
            .unwrap();
        commands
            .import_session(Command::Append {
                operation_id: operation_id.clone(),
                position: 0,
                records: vec![Record {
                    source_message_id: "message".into(),
                    source_turn_id: "turn".into(),
                    timestamp: None,
                    content: Content::User {
                        text: "historical question".into(),
                    },
                }],
            })
            .await
            .unwrap();
        receipt
    } else {
        // The source ceilings were tightened while the Host was stopped. Even
        // a newly authorized capability cannot publish the old wider configuration.
        assert!(matches!(
            commands
                .import_session(Command::Publish {
                    operation_id: operation_id.clone(),
                    records: 1
                })
                .await,
            Err(CommandError::Denied)
        ));
        commands
            .import_session(Command::Inspect {
                operation_id: operation_id.clone(),
            })
            .await
            .unwrap()
    };
    assert_eq!(receipt.progress.state, ImportState::Collecting);
    let catalog = peer
        .rpc(
            "session.catalog.query",
            json!({"kind":"get","sessionId":receipt.session_id}),
        )
        .await;
    assert_eq!(catalog["ok"], true, "{catalog}");
    assert_eq!(
        catalog["result"]["session"],
        serde_json::Value::Null,
        "{catalog}"
    );
    if reopened {
        let abandoned = commands
            .import_session(Command::Abandon { operation_id })
            .await
            .unwrap();
        assert_eq!(abandoned.progress.state, ImportState::Abandoned);
    }
}

pub(super) async fn tighten(fixture: &super::super::support::client_probe::ClientFixture) {
    // Only after the prior Host and all its capabilities have left scope.
    let log = maka_event_log::EventLog::for_root(Arc::new(fixture.owner()))
        .await
        .unwrap();
    let source = log
        .get_session::<maka_runtime_host::session::SessionConfiguration>("scheduled-authority")
        .await
        .unwrap()
        .unwrap();
    log.update_session_metadata(
        "scheduled-authority",
        source.revision,
        |configuration: &mut maka_runtime_host::session::SessionConfiguration| {
            configuration.bound_tools = Some(["Read".to_owned()].into());
            configuration.instructions = Some("new required instruction".into());
            configuration.boundary_revision += 1;
            Ok(())
        },
    )
    .await
    .unwrap();
    log.close().await.unwrap();
}
