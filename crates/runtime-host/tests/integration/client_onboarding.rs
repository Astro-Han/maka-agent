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
use maka_runtime::event::{Fact, InvocationOutcome};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_client_onboards_connections_without_verification_writes_and_reopens_session() {
    let fixture = ClientFixture::new("maka-onboarding-client-");
    let mut canonical = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--onboarding-workspace",
                reopened,
                if reopened {
                    "onboarding-reopened"
                } else {
                    "onboarding-passed"
                },
            )
            .await;
        let log = fixture.log().await;
        let prefix = log.prefix(20_000, 16 * 1024 * 1024).await.unwrap();
        assert_eq!(
            prefix
                .events
                .iter()
                .filter(|stored| matches!(stored.event.fact, Fact::ModelRequested { .. }))
                .count(),
            1,
            "catalog reads, capability replacement and reopen cannot redispatch the model"
        );
        assert_eq!(
            prefix
                .events
                .iter()
                .filter(|stored| matches!(stored.event.fact, Fact::ModelInterrupted { .. }))
                .count(),
            1
        );
        assert!(matches!(
            prefix.events.last().unwrap().event.fact,
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Cancelled { .. }
            }
        ));
        let bytes = serde_json::to_vec(&prefix).unwrap();
        if let Some(before) = &canonical {
            assert_eq!(
                &bytes, before,
                "reopen must preserve the cancelled canonical prefix"
            );
        } else {
            canonical = Some(bytes);
        }
        log.close().await.unwrap();
    }
}
