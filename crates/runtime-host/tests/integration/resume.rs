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
use maka_runtime::{event::Fact, input::InvocationInput};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_resumes_selected_lineage_streams_and_reopens_exact_admission() {
    let fixture = ClientFixture::new("maka-resume-");
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--resume-workspace",
                reopened,
                if reopened {
                    "resume-reopened"
                } else {
                    "resume-passed"
                },
            )
            .await;
        let log = fixture.log().await;
        let prefix = log.prefix(500, 4 * 1024 * 1024).await.unwrap();
        assert_eq!(
            prefix
                .events
                .iter()
                .filter(|s| matches!(s.event.fact, Fact::InvocationOpened { .. }))
                .count(),
            3
        );
        assert_eq!(
            prefix
                .events
                .iter()
                .filter(|s| matches!(
                    s.event.fact,
                    Fact::InvocationOpened {
                        input: InvocationInput::Continuation { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
        let facts = serde_json::to_value(&prefix.events).unwrap();
        for stored in &prefix.events {
            if let Fact::InvocationOpened {
                configuration: Some(configuration),
                ..
            } = &stored.event.fact
            {
                let composition = configuration
                    .tool_composition
                    .as_ref()
                    .expect("Message and manual resume must freeze admitted Host handlers");
                assert!(composition.skills_digest.is_some());
            }
        }
        if let Some(original) = &original {
            assert_eq!(&facts, original);
        } else {
            original = Some(facts);
        }
        log.close().await.unwrap();
    }
}
