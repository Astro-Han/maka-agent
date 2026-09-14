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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_native_connection_test_is_control_plane_and_survives_reopen() {
    let fixture = ClientFixture::new("maka-connection-test-");
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--connection-test-workspace",
                reopened,
                if reopened {
                    "original-client-connection-test-reopened"
                } else {
                    "original-client-connection-test"
                },
            )
            .await;
        let log = fixture.log().await;
        let prefix = log.prefix(1000, 1024 * 1024).await.unwrap();
        assert!(
            prefix.events.is_empty(),
            "connection tests must not create execution facts"
        );
        let bytes = serde_json::to_vec(&prefix).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("dummy-connection-test-secret"));
        if let Some(original) = &original {
            assert_eq!(&bytes, original, "reopen preserves the event log exactly");
        } else {
            original = Some(bytes);
        }
        log.close().await.unwrap();
    }
}
