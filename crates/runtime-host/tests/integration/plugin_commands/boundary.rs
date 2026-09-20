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
    composition::Scope,
    execution::{CommandError, CreateRoot, RootApproval, RootTemplate, SessionBoundary, Submit},
    fiber::Fiber,
    storage::{Data, Mutation},
};
use maka_runtime::execution::ModelBinding;
use maka_runtime_host::server::Host;
use serde_json::json;
use std::{path::Path, sync::Arc, time::Duration};

/// A persisted background plan must not inherit a wider Session grant on restart.
pub(super) async fn verify(
    host: &Arc<Host>,
    peer: &mut Peer,
    model: &ModelBinding,
    workspace: &Path,
    reopened: bool,
) {
    let fiber = Fiber::new("authority-check", "authority-check", Scope::Profile).unwrap();
    fiber.begin_loading().unwrap();
    let storage = host.plugin_storage(fiber.context()).unwrap();
    if !reopened {
        let created = peer.rpc("session.create", json!({
            "sessionId":"scheduled-authority", "workspace":{"kind":"host_path","path":workspace},
            "modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model},
            "permissionMode":"ask"
        })).await;
        assert_eq!(created["ok"], true, "{created}");
        let commands = host
            .authorize_plugin_execution(fiber.context(), &["scheduled-authority".into()])
            .await
            .unwrap();
        let boundaries = commands.boundaries().unwrap();
        assert_eq!(
            boundaries[0].permission_mode,
            maka_runtime::execution::PermissionMode::Ask
        );
        storage
            .batch(vec![Mutation {
                key: "plan-boundaries".into(),
                expected_revision: None,
                data: Data::Present(serde_json::to_value(boundaries).unwrap()),
            }])
            .await
            .unwrap();
        let updated = peer.rpc("session.configuration.update", json!({
            "sessionId":"scheduled-authority", "expectedRevision":created["result"]["revision"], "patch":{"permissionMode":"bypass"}
        })).await;
        assert_eq!(updated["result"]["kind"], "committed", "{updated}");
    }
    let record = storage
        .read("plan-boundaries".into())
        .await
        .unwrap()
        .unwrap();
    let boundaries: Vec<SessionBoundary> =
        serde_json::from_value(record.data.value().unwrap().clone()).unwrap();
    let commands = host
        .restore_plugin_execution(fiber.context(), boundaries.clone())
        .unwrap();
    fiber.ready().unwrap();
    fiber.publish().unwrap();
    assert_eq!(commands.boundaries().unwrap(), boundaries);
    assert!(matches!(
        commands
            .submit(Submit {
                orchestration_mode: None,
                operation_id: "scheduled-attempt".into(),
                session_id: "scheduled-authority".into(),
                content: "must not execute".into()
            })
            .await,
        Err(CommandError::Denied)
    ));
    let current = host
        .authorize_plugin_execution(fiber.context(), &["scheduled-authority".into()])
        .await
        .unwrap();
    assert_eq!(
        current.boundaries().unwrap()[0].permission_mode,
        maka_runtime::execution::PermissionMode::Bypass
    );
    assert_ne!(
        current.boundaries().unwrap()[0].boundary_revision,
        boundaries[0].boundary_revision
    );
    let request = CreateRoot {
        managed: true,
        operation_id: "frozen-root".into(),
        name: "Scheduled run".into(),
        settings: maka_plugins::execution::RootSettings {
            target: maka_plugins::execution::Target::Model {
                model: model.clone(),
                thinking_level: None,
            },
            permission_mode: maka_runtime::execution::PermissionMode::Explore,
            tool_mode: maka_runtime::execution::ToolMode::CodeMode,
            collaboration_mode: maka_runtime::execution::CollaborationMode::Agent,
            behavior: maka_runtime::execution::BehaviorId::default(),
            bound_tools: None,
            instructions: None,
        },
    };
    assert!(matches!(
        current.create_root(request.clone()).await,
        Err(CommandError::Denied)
    ));
    let root_workspace = workspace.parent().unwrap().join("root-workspace");
    std::fs::create_dir_all(&root_workspace).unwrap();
    let cwd = maka_fs_tools::workspace::project::host_path(&root_workspace.canonicalize().unwrap())
        .unwrap()
        .to_owned();
    let template = RootTemplate {
        workspace: maka_runtime::execution::WorkspaceTarget::HostPath { path: cwd.clone() },
        cwd,
        workspace_identity: maka_fs_tools::workspace::ensure_identity(&root_workspace)
            .await
            .unwrap(),
        model: model.clone(),
        thinking_level: None,
        tool_mode: maka_runtime::execution::ToolMode::CodeMode,
        permission_mode: maka_runtime::execution::PermissionMode::Explore,
        collaboration_mode: maka_runtime::execution::CollaborationMode::Agent,
        orchestration_mode: maka_runtime::execution::BehaviorId::default(),
    };
    let roots = host
        .authorize_plugin_root_execution(
            fiber.context(),
            RootApproval {
                template,
                source: None,
            },
        )
        .unwrap();
    let root = roots.create_root(request.clone()).await.unwrap();
    assert_eq!(roots.create_root(request.clone()).await.unwrap(), root);
    let current = roots.session(root.session_id.clone()).await.unwrap();
    let configured = roots
        .configure(maka_plugins::execution::Configure {
            session_id: root.session_id.clone(),
            expected_revision: current.revision,
            target: current.target.clone(),
        })
        .await
        .unwrap();
    let maka_plugins::execution::Configured::Committed { session } = configured else {
        panic!("configuration CAS rejected the current revision");
    };
    assert!(session.revision >= current.revision);
    // Creation's immutable receipt identifies the original intent; it must not
    // forbid a later authorized model selection from restoring the same root.
    assert_eq!(roots.create_root(request.clone()).await.unwrap(), root);
    assert!(matches!(
        roots
            .configure(maka_plugins::execution::Configure {
                session_id: "foreign".into(),
                expected_revision: session.revision,
                target: current.target,
            })
            .await,
        Err(CommandError::Denied)
    ));
    assert!(matches!(
        roots
            .create_root(CreateRoot {
                name: "different".into(),
                ..request
            })
            .await,
        Err(CommandError::Conflict)
    ));
    if !reopened {
        storage
            .batch(vec![Mutation {
                key: "created-root".into(),
                expected_revision: None,
                data: Data::Present(serde_json::to_value(&root).unwrap()),
            }])
            .await
            .unwrap();
    } else {
        let saved = storage.read("created-root".into()).await.unwrap().unwrap();
        assert_eq!(
            saved.data.value().unwrap(),
            &serde_json::to_value(&root).unwrap()
        );
    }
    let queried = peer
        .rpc(
            "session.catalog.query",
            json!({"kind":"get","sessionId":root.session_id}),
        )
        .await;
    assert_eq!(queried["ok"], true, "{queried}");
    assert_eq!(
        queried["result"]["session"]["permissionMode"], "explore",
        "{queried}"
    );
    assert_eq!(roots.boundaries().unwrap().len(), 1);
    let changed = peer.rpc("session.configuration.update", json!({
        "sessionId": root.session_id, "expectedRevision": queried["result"]["session"]["revision"],
        "patch": { "permissionMode": "bypass" }
    })).await;
    assert_eq!(
        changed["ok"], false,
        "managed root must reject ordinary mutation: {changed}"
    );
    let foreign = Fiber::new("other-package", "other-entry", Scope::Profile).unwrap();
    foreign.begin_loading().unwrap();
    foreign.ready().unwrap();
    foreign.publish().unwrap();
    let commands = host
        .authorize_plugin_execution(foreign.context(), std::slice::from_ref(&root.session_id))
        .await
        .unwrap();
    assert!(matches!(
        commands.session(root.session_id).await,
        Err(CommandError::Denied)
    ));
    foreign
        .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    fiber
        .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
}
