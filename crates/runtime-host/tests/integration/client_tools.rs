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

use super::support::client_probe::ClientFixture;
use maka_runtime::event::Fact;
use maka_runtime::execution::{PermissionMode, ToolMode};
use maka_runtime_host::session::SessionConfiguration;
use sha2::{Digest, Sha256};
use std::path::Path;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_client_reads_scoped_file_and_reopens_identical_facts_and_rows() {
    let fixture = ClientFixture::new("maka-read-");
    let workspace = &fixture.workspace;
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--tool-workspace",
                reopened,
                if reopened {
                    "original-client-read-reopened"
                } else {
                    "original-client-read"
                },
            )
            .await;
        let log = fixture.log().await;
        let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
        let current = log
            .get_session::<SessionConfiguration>("read-session")
            .await
            .unwrap()
            .unwrap();
        let original_cwd = workspace.canonicalize().unwrap();
        assert_ne!(
            current.configuration.workspace.host_cwd,
            original_cwd.to_str().unwrap()
        );
        let mut openings = 0;
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(workspace.join("read-rows.json")).unwrap())
                .unwrap();
        let mut dispatched = 0;
        let live: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(workspace.join("read-live.json")).unwrap())
                .unwrap();
        for stored in &prefix.events {
            if let Fact::InvocationOpened { configuration, .. } = &stored.event.fact {
                openings += 1;
                let configuration = configuration
                    .as_ref()
                    .expect("Host opening must freeze its context");
                assert_eq!(configuration.cwd, current.configuration.workspace.host_cwd);
                assert_eq!(
                    configuration.workspace_identity.as_ref(),
                    Some(
                        &maka_fs_tools::workspace::read_identity(Path::new(&configuration.cwd))
                            .unwrap()
                    )
                );
                assert_eq!(configuration.permission_mode, PermissionMode::Explore);
                assert_eq!(configuration.tool_mode, ToolMode::Direct);
                assert_eq!(
                    configuration.model.as_ref(),
                    current.configuration.target.model()
                );
            }
            if let Fact::ToolDispatched { operation_id, .. } = &stored.event.fact {
                dispatched += 1;
                let invocation = &stored.event.invocation;
                let tuple = serde_json::to_vec(&[
                    "maka.tool-presentation.v1",
                    &invocation.invocation_id,
                    operation_id,
                ])
                .unwrap();
                let expected = format!("tool_{:x}", Sha256::digest(tuple));
                let start = live
                    .iter()
                    .find(|event| event["type"] == "tool_start" && event["toolUseId"] == expected)
                    .unwrap();
                assert_eq!(
                    start["ts"].as_u64().unwrap(),
                    stored
                        .event
                        .recorded_at
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64
                );
                assert!(rows.iter().any(|row| row["type"] == "tool_call"
                    && row["turnId"] == invocation.turn_id
                    && row["id"] == expected));
            }
        }
        assert_eq!(dispatched, 2);
        assert_eq!(openings, 2);
        let bytes = serde_json::to_vec(&prefix).unwrap();
        if let Some(original) = &original {
            assert_eq!(
                &bytes, original,
                "reopen must preserve canonical facts and raw-byte digest"
            );
        } else {
            original = Some(bytes);
        }
        log.close().await.unwrap();
    }
}
