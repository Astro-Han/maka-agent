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
use maka_runtime::{
    event::{Fact, ToolOutcome},
    execution::ThinkingLevel,
    model::{ModelPart, TextKind},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_compatible_profiles_reach_http_and_reopen_exact_facts() {
    let fixture = ClientFixture::new("maka-compatible-");
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--compatible-workspace",
                reopened,
                if reopened {
                    "compatible-options-reopened"
                } else {
                    "compatible-options-passed"
                },
            )
            .await;
        let log = fixture.log().await;
        let prefix = log.prefix(1000, 1024 * 1024).await.unwrap();
        let mut openings = Vec::new();
        let mut thinking = Vec::new();
        let mut dispatches = std::collections::BTreeSet::new();
        let mut settled = 0;
        for stored in &prefix.events {
            match &stored.event.fact {
                Fact::InvocationOpened { configuration, .. } => {
                    openings.push(configuration.as_ref().unwrap().thinking_level);
                }
                Fact::ModelCompleted { output, .. } => {
                    for part in &output.parts {
                        if let ModelPart::Text {
                            text_kind: TextKind::Thinking,
                            text,
                            provider_options,
                        } = part
                        {
                            let field = provider_options.as_ref().unwrap()["maka"]["openAiChatReasoningField"]
                                .as_str().unwrap();
                            thinking.push((field.to_owned(), text.clone()));
                        }
                    }
                    if output.tool_calls().next().is_some() {
                        let texts: Vec<_> = output
                            .parts
                            .iter()
                            .filter_map(|part| match part {
                                ModelPart::Text {
                                    text_kind, text, ..
                                } => Some((*text_kind, text.as_str())),
                                _ => None,
                            })
                            .collect();
                        assert_eq!(
                            texts,
                            if stored
                                .event
                                .invocation
                                .session_id
                                .ends_with("reasoning_content")
                            {
                                vec![(TextKind::Thinking, ""), (TextKind::Text, "before after")]
                            } else {
                                vec![
                                    (TextKind::Thinking, "first "),
                                    (TextKind::Text, "before "),
                                    (TextKind::Thinking, "second"),
                                    (TextKind::Text, "after"),
                                ]
                            }
                        );
                    }
                }
                Fact::ToolDispatched { operation_id, .. } => {
                    assert!(dispatches.insert(operation_id.clone()));
                }
                Fact::ToolSettled {
                    operation_id,
                    outcome,
                } => {
                    assert!(dispatches.remove(operation_id), "T2 follows durable T1");
                    assert!(matches!(outcome, ToolOutcome::Succeeded { .. }));
                    settled += 1;
                }
                _ => {}
            }
        }
        let mut expected_openings = vec![
            Some(ThinkingLevel::High),
            Some(ThinkingLevel::Max),
            Some(ThinkingLevel::High),
            Some(ThinkingLevel::Max),
        ];
        if reopened {
            expected_openings.extend([Some(ThinkingLevel::Max); 2]);
        }
        assert_eq!(openings, expected_openings);
        assert_eq!(settled, if reopened { 6 } else { 4 });
        assert!(dispatches.is_empty());
        let mut expected_thinking = vec![
            ("reasoning".into(), "first ".into()),
            ("reasoning".into(), "second".into()),
            ("reasoning".into(), "first ".into()),
            ("reasoning".into(), "second".into()),
            ("reasoning_content".into(), "".into()),
            ("reasoning_content".into(), "".into()),
        ];
        if reopened {
            expected_thinking.extend([
                ("reasoning".into(), "first ".into()),
                ("reasoning".into(), "second".into()),
                ("reasoning_content".into(), "".into()),
            ]);
        }
        assert_eq!(thinking, expected_thinking);
        if let Some((count, bytes)) = &original {
            assert!(prefix.events.len() > *count);
            assert_eq!(
                &serde_json::to_vec(&prefix.events[..*count]).unwrap(),
                bytes,
                "new turns after reopen preserve the exact original canonical prefix"
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
