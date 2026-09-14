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
use maka_runtime::{
    event::{Fact, ToolOutcome},
    tool_output::{DurableToolProjection, ToolOutput},
};
use maka_runtime_host::server::{Host, local::LocalListener};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_attachment_results_preserve_raw_without_poisoning_model_history() {
    let directory = tempfile::Builder::new()
        .prefix("maka-large-output-")
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
                .arg("--large-output-workspace")
                .arg(client_workspace);
            if reopened {
                command.arg("--reopened");
            }
            command.output().unwrap()
        });
        let output = tokio::time::timeout(Duration::from_secs(150), client)
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
                "large-output-reopened"
            } else {
                "large-output-passed"
            })
        );

        let log = EventLog::open(&root.join(maka_event_log::root::ROOT_DATABASE))
            .await
            .unwrap();
        // Both 10 MiB results, including a roughly 60 MiB JSON encoding, stay
        // outside the compact model/recovery evidence budget.
        let prefix = log.prefix(1000, 1024 * 1024).await.unwrap();
        let mut results = std::collections::BTreeSet::new();
        for stored in &prefix.events {
            let Fact::ToolSettled {
                outcome:
                    ToolOutcome::Succeeded {
                        raw,
                        model_projection,
                    },
                ..
            } = &stored.event.fact
            else {
                continue;
            };
            let session = &stored.event.invocation.session_id;
            assert!(
                results.insert(session.clone()),
                "no repeated effect after reopen"
            );
            let expected_byte = match session.as_str() {
                "large-output-ascii" => 65,
                "large-output-control" => 1,
                unexpected => panic!("unexpected tool result Session {unexpected}"),
            };
            let DurableToolProjection::Json { value: page } = model_projection else {
                panic!("Read must freeze a bounded page")
            };
            assert!(page.to_string().encode_utf16().count() <= maka_runtime::read::MAX_PAGE_CHARS);
            let content = page["content"].as_str().unwrap();
            assert!(!content.is_empty());
            assert!(content.bytes().all(|byte| byte == expected_byte));
            assert_eq!(page["partialLine"], true);
            assert!(
                page["next"]["path"]
                    .as_str()
                    .unwrap()
                    .starts_with("maka://read/")
            );
            assert!(raw.bytes > 10 * 1024 * 1024);
            if expected_byte == 1 {
                assert!(raw.bytes > 16 * 1024 * 1024);
            }
            let ToolOutput::Text(text) = log
                .resolve_tool_result(session, &stored.event.id)
                .await
                .unwrap()
            else {
                panic!("expected raw text evidence");
            };
            assert_eq!(text.len(), 10 * 1024 * 1024);
            assert!(text.bytes().all(|byte| byte == expected_byte));
            assert!(
                prefix
                    .project_invocation(&stored.event.invocation.invocation_id)
                    .uncertain_operations
                    .is_empty()
            );
        }
        assert_eq!(results.len(), 2);
        if let Some((count, bytes)) = &original {
            assert!(
                prefix.events.len() > *count,
                "reopen fixture runs new model turns"
            );
            assert_eq!(
                &serde_json::to_vec(&prefix.events[..*count]).unwrap(),
                bytes,
                "new turns cannot rewrite prior raw references, projections or identities"
            );
        } else {
            original = Some((
                prefix.events.len(),
                serde_json::to_vec(&prefix.events).unwrap(),
            ));
        }
        log.close().await.unwrap();
    }
}
