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
use sha2::{Digest, Sha256};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_client_patches_reads_deletes_and_reopens_identical_facts() {
    let fixture = ClientFixture::new("maka-patch-");
    let workspace = &fixture.workspace;
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--patch-workspace",
                reopened,
                if reopened {
                    "original-client-patch-reopened"
                } else {
                    "original-client-patch"
                },
            )
            .await;
        let log = fixture.log().await;
        let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(workspace.join("patch-rows.json")).unwrap())
                .unwrap();
        let mut dispatched = 0;
        let mut settled = 0;
        let mut rejected = 0;
        let live: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(workspace.join("patch-live.json")).unwrap())
                .unwrap();
        for stored in &prefix.events {
            match &stored.event.fact {
                Fact::ToolSettled { .. } => settled += 1,
                Fact::ToolRejected { .. } => rejected += 1,
                _ => {}
            }
            if let Fact::ToolDispatched { operation_id, .. } = &stored.event.fact {
                dispatched += 1;
                assert_eq!(stored.event.invocation.session_id, "patch-ask");
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
        assert_eq!((dispatched, settled, rejected), (4, 4, 0));
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
