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

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use maka_event_log::EventLog;
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::{
    event::Fact,
    execution::{SandboxMode, ToolMode},
};
use maka_runtime_host::server::{Host, local::LocalListener};
use maka_runtime_host::session::SessionConfiguration;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_client_configures_session_checks_cas_and_reopens_durable_state() {
    let directory = tempfile::Builder::new()
        .prefix("maka-interop-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let ns = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let root = directory.path().join("root");
    let owner = RootOwner::create(&root, &ns).unwrap();
    let root_id = owner.root_id().to_owned();
    drop(owner);
    let mut original = None;
    for reopened in [false, true] {
        let owner = RootOwner::open(&root, &ns).unwrap();
        assert_eq!(owner.root_id(), root_id);
        let host = Host::open(owner).await.unwrap();
        let socket = directory.path().join("h.sock");
        let listener = LocalListener::bind(&socket).unwrap();
        let cancellation = CancellationToken::new();
        let server = tokio::spawn(listener.serve(host, cancellation.clone()));
        let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs");
        let workspace = directory.path().to_owned();
        let expected_id = root_id.clone();
        let client = tokio::task::spawn_blocking(move || {
            let mut command = Command::new("node");
            command
                .arg(probe)
                .arg("--socket")
                .arg(socket)
                .args(["--root-id", &expected_id])
                .arg("--session-workspace")
                .arg(workspace);
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
        let check = if reopened {
            "session-control-reopen"
        } else {
            "session-control-workflow"
        };
        assert!(String::from_utf8_lossy(&output.stdout).contains(check));
        assert!(String::from_utf8_lossy(&output.stdout).contains("original-client-read-marker"));
        let log = EventLog::open(&root.join(maka_event_log::root::ROOT_DATABASE))
            .await
            .unwrap();
        let prefix = log.prefix(1000, 1024 * 1024).await.unwrap();
        let first = prefix
            .events
            .iter()
            .find(|stored| {
                stored.event.invocation.turn_id == "first-turn"
                    && matches!(stored.event.fact, Fact::InvocationOpened { .. })
            })
            .unwrap();
        let Fact::InvocationOpened {
            input: maka_runtime::input::InvocationInput::Message { content, .. },
            ..
        } = &first.event.fact
        else {
            panic!("original user input must remain canonical");
        };
        assert_eq!(content.text, "first question");
        assert_eq!(content.quotes.as_ref().unwrap().len(), 2);
        assert_eq!(
            content.quotes.as_ref().unwrap()[0]
                .source_turn_id
                .as_deref(),
            Some("older-turn")
        );
        assert_eq!(
            content.directory_references.as_ref().unwrap()[0].host_id,
            root_id
        );
        assert_eq!(
            content.inline_references.as_ref().unwrap()[0].value,
            "@source.rs"
        );
        let current = log
            .get_session::<SessionConfiguration>("rust-interop-session")
            .await
            .unwrap()
            .unwrap();
        let original_cwd = directory.path().canonicalize().unwrap();
        assert_ne!(
            current.configuration.workspace.host_cwd,
            original_cwd.to_str().unwrap()
        );
        let openings: Vec<_> = prefix
            .events
            .iter()
            .filter_map(|stored| {
                if let Fact::InvocationOpened { configuration, .. } = &stored.event.fact {
                    Some((
                        &stored.event.invocation.turn_id,
                        configuration
                            .as_ref()
                            .expect("Host opening must freeze its context"),
                    ))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(openings.len(), 3);
        for (turn, configuration) in openings {
            assert_eq!(
                configuration.cwd,
                original_cwd.to_str().unwrap(),
                "later relocation cannot reinterpret historical execution cwd"
            );
            assert_eq!(
                configuration.sandbox_mode,
                if turn == "first-turn" {
                    SandboxMode::ReadOnly
                } else {
                    SandboxMode::WorkspaceWrite
                },
                "opening must retain the grants actually used by this Turn"
            );
            assert_eq!(configuration.tool_mode, ToolMode::Direct);
            assert_eq!(
                configuration.model.as_ref(),
                current.configuration.target.model()
            );
        }
        let bytes = serde_json::to_vec(&prefix).unwrap();
        if let Some(original) = &original {
            assert_eq!(
                &bytes, original,
                "read ack and restart preserve canonical log bytes"
            );
        } else {
            original = Some(bytes);
        }
        log.close().await.unwrap();
    }
}
