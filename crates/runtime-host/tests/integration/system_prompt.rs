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
use serde_json::Value;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_prompt_sources_are_frozen_per_step_and_preserved_across_reopen() {
    let fixture = ClientFixture::new("maka-system-prompt-");
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--system-prompt-workspace",
                reopened,
                if reopened {
                    "system-prompt-reopened"
                } else {
                    "system-prompt-passed"
                },
            )
            .await;
        let saved: Value = serde_json::from_slice(
            &std::fs::read(fixture.workspace.join("system-prompt-fixture.json")).unwrap(),
        )
        .unwrap();
        let log = fixture.log().await;
        let prefix = log.prefix(200, 1024 * 1024).await.unwrap();
        let mut openings = Vec::new();
        let mut model_steps = 0;
        for stored in &prefix.events {
            if let Fact::InvocationOpened {
                configuration: Some(configuration),
                ..
            } = &stored.event.fact
            {
                assert!(
                    configuration.system_prompt.is_none(),
                    "default persona is not a hidden Run baseline"
                );
                openings.push(stored.sequence);
            }
            if matches!(stored.event.fact, Fact::ModelRequested { .. }) {
                assert!(
                    openings
                        .last()
                        .is_some_and(|sequence| *sequence < stored.sequence)
                );
                let composition = log
                    .request_composition(&stored.event.invocation.session_id, &stored.event.id)
                    .await
                    .unwrap()
                    .unwrap();
                let index = if model_steps == 4 { 2 } else { model_steps };
                assert_eq!(
                    composition.system_prompt.as_deref(),
                    saved["prompts"][index].as_str()
                );
                assert!(
                    composition
                        .sources
                        .iter()
                        .any(|source| source.package_id == "maka.assistant")
                );
                model_steps += 1;
            }
        }
        assert_eq!(openings.len(), if reopened { 4 } else { 3 });
        assert_eq!(model_steps, if reopened { 5 } else { 4 });
        let facts = serde_json::to_value(&prefix.events).unwrap();
        if let Some(original) = &original {
            let original: &Vec<Value> = original;
            assert_eq!(&facts.as_array().unwrap()[..original.len()], original);
        } else {
            original = Some(facts.as_array().unwrap().clone());
        }
        log.close().await.unwrap();
    }
}
