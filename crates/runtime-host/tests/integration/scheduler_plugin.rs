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
mod execution;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn original_client_scheduler_pages_native_admission_disable_and_restart() {
    let fixture = ClientFixture::new("maka-scheduler-plugin-");
    // An existing root knows only the older built-in layer; upgrade must add
    // Scheduler defaults without replacing the stored composition.
    let log = fixture.log().await;
    log.commit_plugin_state(
        maka_plugins::composition::Ledger {
            package_layers: vec!["maka.agent-graph".into()],
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    log.close().await.unwrap();
    for reopened in [false, true] {
        fixture
            .run(
                "--scheduler-workspace",
                reopened,
                "original-client-scheduler",
            )
            .await;
    }
}
