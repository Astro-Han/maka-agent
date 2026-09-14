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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_controls_durable_queue_and_reopens_cancellation_proofs() {
    let fixture = ClientFixture::new("maka-message-queue-");
    // Create live admissions through the unchanged client. Startup now consumes
    // pending work, so a synthetic idle queue is no longer a valid Host fixture.
    fixture
        .run("--message-queue-workspace", false, "message-queue-passed")
        .await;
    let log = fixture.log().await;
    assert!(log.pending_messages("queue").await.unwrap().is_empty());
    for id in ["one", "two", "steer"] {
        assert!(log.message_cancelled("queue", id).await.unwrap());
        assert!(log.root_message("queue", id).await.unwrap().is_none());
    }
    for id in ["root-message", "active-message"] {
        assert!(log.root_message("queue", id).await.unwrap().is_some());
    }
    let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
    assert_eq!(
        prefix
            .events
            .iter()
            .filter(|event| matches!(event.event.fact, Fact::InvocationOpened { .. }))
            .count(),
        2
    );
    let prefix = serde_json::to_vec(&prefix).unwrap();
    let revision = log.message_queue("queue").await.unwrap().revision;
    log.close().await.unwrap();
    fixture
        .run("--message-queue-workspace", true, "message-queue-reopened")
        .await;
    let log = fixture.log().await;
    assert_eq!(log.message_queue("queue").await.unwrap().revision, revision);
    assert_eq!(
        serde_json::to_vec(&log.prefix(100, 1024 * 1024).await.unwrap()).unwrap(),
        prefix
    );
    log.close().await.unwrap();
}
