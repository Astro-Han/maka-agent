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

use maka_event_log::{
    EventLog,
    root::{RootNamespaces, RootOwner},
};
use maka_runtime::event::{Fact, InvocationOutcome};
use maka_runtime_host::server::{Host, local::LocalListener};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

/// Explicit opt-in: one real free-router request, no retry or paid fallback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OPENROUTER_API_KEY and external OpenRouter free-route availability"]
async fn original_client_live_openrouter_stream_is_durable_and_not_replayed_on_reopen() {
    assert!(
        std::env::var("OPENROUTER_API_KEY").is_ok_and(|key| !key.is_empty()),
        "OPENROUTER_API_KEY must be set for the explicitly requested live test"
    );
    let directory = tempfile::Builder::new()
        .prefix("maka-live-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let ns = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let root = directory.path().join("root");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let owner = RootOwner::create(&root, &ns).unwrap();
    let root_id = owner.root_id().to_owned();
    drop(owner);
    let mut original = None;
    for reopened in [false, true] {
        let host = Host::open(RootOwner::open(&root, &ns).unwrap())
            .await
            .unwrap();
        let socket = directory.path().join("h.sock");
        let listener = LocalListener::bind(&socket).unwrap();
        let cancellation = CancellationToken::new();
        let server = tokio::spawn(listener.serve(host, cancellation.clone()));
        let expected_id = root_id.clone();
        let client_workspace = workspace.clone();
        let client = tokio::task::spawn_blocking(move || {
            let mut command = Command::new("node");
            command
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs"))
                .arg("--socket")
                .arg(socket)
                .args(["--root-id", &expected_id])
                .arg("--live-openrouter-workspace")
                .arg(client_workspace);
            if reopened {
                command.arg("--reopened").env_remove("OPENROUTER_API_KEY");
            }
            command.output().unwrap()
        });
        // The probe owns its 140 s deadline and closes the transport on failure.
        let output = client.await.unwrap();
        cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let log = EventLog::open(&root.join(maka_event_log::root::ROOT_DATABASE))
            .await
            .unwrap();
        let prefix = log.prefix(10000, 8 * 1024 * 1024).await.unwrap();
        let terminal = prefix
            .events
            .iter()
            .find_map(|stored| match &stored.event.fact {
                Fact::InvocationEnded { outcome } => Some(outcome),
                _ => None,
            });
        let key = std::env::var("OPENROUTER_API_KEY").unwrap();
        let bytes = serde_json::to_vec(&prefix).unwrap();
        for evidence in [&output.stdout, &output.stderr, &bytes] {
            assert!(
                !String::from_utf8_lossy(evidence).contains(&key),
                "credential escaped its private configuration boundary"
            );
        }
        assert!(
            output.status.success(),
            "client stdout: {}\nclient stderr: {}\nterminal: {terminal:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(terminal, Some(&InvocationOutcome::Completed));
        assert_eq!(
            prefix
                .events
                .iter()
                .filter(|stored| matches!(stored.event.fact, Fact::ModelRequested { .. }))
                .count(),
            1
        );
        let outputs: Vec<_> = prefix
            .events
            .iter()
            .filter_map(|stored| match &stored.event.fact {
                Fact::ModelCompleted { output, .. } => Some(output),
                _ => None,
            })
            .collect();
        assert_eq!(outputs.len(), 1);
        assert!(
            outputs[0]
                .model
                .as_ref()
                .is_some_and(|model| !model.is_empty())
        );
        assert!(
            outputs[0]
                .usage
                .output_tokens
                .is_some_and(|tokens| tokens > 0)
        );
        println!(
            "real free-router backend: {}",
            outputs[0].model.as_deref().unwrap()
        );
        if let Some(original) = &original {
            assert_eq!(
                &bytes, original,
                "reopen must not replay a paid/external effect"
            );
        } else {
            original = Some(bytes);
        }
        log.close().await.unwrap();
    }
    // TempDir removes only this test's credential vault, root, sockets and transcript.
    directory.close().unwrap();
}
