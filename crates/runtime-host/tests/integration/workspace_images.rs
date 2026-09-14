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

#![cfg(unix)]

use base64::{Engine as _, engine::general_purpose::STANDARD};
use maka_event_log::{
    EventLog,
    root::{ROOT_DATABASE, RootNamespaces, RootOwner},
};
use maka_runtime::{
    artifact::ArtifactSource,
    attachment::StorageRef,
    event::{Fact, ToolOutcome},
    tool_output::ToolOutput,
};
use maka_runtime_host::server::{Host, local::LocalListener};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_image_snapshot_survives_source_removal_and_host_reopen() {
    let directory = tempfile::Builder::new()
        .prefix("maka-workspace-image-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let namespaces = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let root = directory.path().join("root");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let owner = RootOwner::create(&root, &namespaces).unwrap();
    let root_id = owner.root_id().to_owned();
    drop(owner);
    let expected_image = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aWQAAAABJRU5ErkJggg==").unwrap();
    let mut original = None;
    for reopened in [false, true] {
        let host = Host::open(RootOwner::open(&root, &namespaces).unwrap())
            .await
            .unwrap();
        let socket = directory.path().join("h.sock");
        let cancellation = CancellationToken::new();
        let server = tokio::spawn(
            LocalListener::bind(&socket)
                .unwrap()
                .serve(host, cancellation.clone()),
        );
        let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs");
        let client_workspace = workspace.clone();
        let expected_id = root_id.clone();
        let client = tokio::task::spawn_blocking(move || {
            let mut command = Command::new("node");
            command
                .arg(probe)
                .arg("--socket")
                .arg(socket)
                .args(["--root-id", &expected_id])
                .arg("--workspace-image-workspace")
                .arg(client_workspace);
            if reopened {
                command.arg("--reopened");
            }
            command.output().unwrap()
        });
        let output = tokio::time::timeout(Duration::from_secs(60), client)
            .await
            .unwrap()
            .unwrap();
        cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(if reopened {
                "workspace-image-reopened"
            } else {
                "workspace-image-passed"
            })
        );
        let log = EventLog::open(&root.join(ROOT_DATABASE)).await.unwrap();
        let prefix = log.prefix(500, 1024 * 1024).await.unwrap();
        let mut images = 0;
        for stored in &prefix.events {
            if !matches!(
                stored.event.fact,
                Fact::ToolSettled {
                    outcome: ToolOutcome::Succeeded { .. },
                    ..
                }
            ) {
                continue;
            }
            let invocation = &stored.event.invocation;
            let raw = log
                .resolve_tool_result(&invocation.session_id, &stored.event.id)
                .await
                .unwrap();
            let ToolOutput::Image(image) = raw else {
                continue;
            };
            images += 1;
            assert_eq!(image.mime_type, "image/png");
            let StorageRef::SessionFile {
                session_id,
                relative_path,
            } = image.reference
            else {
                panic!("workspace bytes must become a durable Session artifact");
            };
            assert_eq!(session_id, invocation.session_id);
            let artifact = log
                .get_artifact(&session_id, &relative_path)
                .await
                .unwrap()
                .record
                .unwrap();
            assert_eq!(artifact.source, ArtifactSource::ToolResultProjection);
            assert_eq!(artifact.turn_id, invocation.turn_id);
            let payload = log
                .read_artifact_chunk(&session_id, &relative_path, 0, 1024)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(payload.bytes, expected_image);
        }
        assert_eq!(
            images, 1,
            "restart and retry must not repeat the Read effect"
        );
        let facts = serde_json::to_value(&prefix.events).unwrap();
        if let Some(original) = &original {
            let original: &Vec<serde_json::Value> = original;
            assert_eq!(&facts.as_array().unwrap()[..original.len()], original);
        } else {
            original = Some(facts.as_array().unwrap().clone());
        }
    }
}
