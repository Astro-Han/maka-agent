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
use maka_runtime::{event::Fact, execution::ThinkingLevel};
use maka_runtime_host::{
    server::{Host, local::LocalListener},
    session::SessionConfiguration,
};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_native_anthropic_options_and_signed_history_survive_reopen() {
    let directory = tempfile::Builder::new()
        .prefix("maka-anthropic-")
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
        let client_workspace = workspace.clone();
        let expected_id = root_id.clone();
        let client = tokio::task::spawn_blocking(move || {
            let mut command = Command::new("node");
            command
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs"))
                .arg("--socket")
                .arg(socket)
                .args(["--root-id", &expected_id])
                .arg("--anthropic-options-workspace")
                .arg(client_workspace);
            if reopened {
                command.arg("--reopened");
            }
            command.output().unwrap()
        });
        let output = tokio::time::timeout(Duration::from_secs(30), client)
            .await
            .unwrap()
            .unwrap();
        cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(5), server)
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
                "original-client-anthropic-options-reopened"
            } else {
                "original-client-anthropic-options"
            })
        );
        let log = EventLog::open(&root.join(maka_event_log::root::ROOT_DATABASE))
            .await
            .unwrap();
        let prefix = log.prefix(1000, 1024 * 1024).await.unwrap();
        for (model, selected) in [
            ("claude-sonnet-4-5", ThinkingLevel::Off),
            ("claude-opus-4-6", ThinkingLevel::High),
        ] {
            let current = log
                .get_session::<SessionConfiguration>(model)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(current.configuration.thinking_level, None);
            let levels: Vec<_> = prefix
                .events
                .iter()
                .filter_map(|stored| {
                    let Fact::InvocationOpened { configuration, .. } = &stored.event.fact else {
                        return None;
                    };
                    if !stored.event.invocation.turn_id.starts_with(model) {
                        return None;
                    }
                    let frozen = configuration
                        .as_ref()
                        .expect("opening freezes configuration");
                    assert_eq!(frozen.model.as_ref(), current.configuration.target.model());
                    Some(frozen.thinking_level)
                })
                .collect();
            assert_eq!(
                levels,
                [None, Some(selected), None],
                "updates must not rewrite earlier invocation contexts"
            );
        }
        let bytes = serde_json::to_vec(&prefix).unwrap();
        if let Some(original) = &original {
            assert_eq!(
                &bytes, original,
                "reopen preserves canonical facts and raw-byte digest"
            );
        } else {
            original = Some(bytes);
        }
        log.close().await.unwrap();
    }
}
