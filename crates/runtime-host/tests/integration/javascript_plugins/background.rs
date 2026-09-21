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

use super::{ClientFixture, Host, Peer, package, ready};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn pending_work_owns_residency_and_wake_can_replace_itself_without_deadlock() {
    tokio::time::timeout(Duration::from_secs(20), async {
        for mode in ["shared", "dedicated"] {
            let fixture = ClientFixture::new("maka-background-wake-");
            let package = package(
                &fixture.workspace,
                "example.pending",
                mode,
                r#"
                export default async function(ctx) {
                    let registration = await ctx.background.pending('pending', async () => {
                        await registration.close();
                        registration = await ctx.background.pending('pending',
                            () => registration.close());
                    });
                }
            "#,
                false,
            );
            let host = Host::open(fixture.owner()).await.unwrap();
            #[cfg(unix)]
            let endpoint = fixture.workspace.parent().unwrap().join("pending.sock");
            #[cfg(windows)]
            let endpoint = std::path::PathBuf::from(format!(
                r"\\.\pipe\maka-pending-{}",
                uuid::Uuid::new_v4()
            ));
            let stop = tokio_util::sync::CancellationToken::new();
            let cleanup = stop.clone().drop_guard();
            let server = tokio::spawn(
                maka_runtime_host::server::local::LocalListener::bind(&endpoint)
                    .unwrap()
                    .serve(host.clone(), stop.clone()),
            );
            let mut peer = Peer::new(host.clone(), "background-wake").await;
            let installed = peer
                .rpc("plugin.package.install", json!({"sourcePath":package}))
                .await;
            assert_eq!(installed["ok"], true, "{installed}");
            ready(&mut peer).await;
            peer.close().await;
            assert_pending(&host).await;

            // The first wake replaces its own registration; only the second ends the work.
            for pending in [true, false] {
                let mut peer = Peer::new(host.clone(), "background-wake").await;
                let result = peer.rpc("host.wake", json!({})).await;
                assert_eq!(result["ok"], true, "{result}");
                peer.close().await;
                if pending {
                    assert_pending(&host).await;
                } else {
                    tokio::time::timeout(
                        Duration::from_secs(3),
                        host.wait_until_idle(Duration::ZERO, Duration::ZERO),
                    )
                    .await
                    .unwrap();
                }
            }

            // Re-activation restores plugin-owned work; disabling removes its residency.
            for disabled in [true, false, true] {
                let mut peer = Peer::new(host.clone(), "background-wake").await;
                let result = peer.rpc("plugin.composition.apply", json!({"operations":[
                    {"type":"update","entryId":"example.pending","patch":{"disabled":disabled}}
                ]})).await;
                assert_eq!(result["ok"], true, "{result}");
                ready(&mut peer).await;
                peer.close().await;
                if !disabled {
                    assert_pending(&host).await;
                }
            }
            tokio::time::timeout(
                Duration::from_secs(3),
                host.wait_until_idle(Duration::ZERO, Duration::ZERO),
            )
            .await
            .unwrap();
            stop.cancel();
            server.await.unwrap().unwrap();
            cleanup.disarm();
        }
    })
    .await
    .expect("background registrations must make bounded progress");
}

async fn assert_pending(host: &Arc<Host>) {
    assert!(
        tokio::time::timeout(
            Duration::from_millis(350),
            host.wait_until_idle(Duration::ZERO, Duration::from_millis(100))
        )
        .await
        .is_err(),
        "pending plugin work must survive client disconnection"
    );
}
