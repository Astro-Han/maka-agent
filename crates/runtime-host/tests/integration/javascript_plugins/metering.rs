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

use maka_event_log::{
    EventLog,
    transcript::{TranscriptDirection, TranscriptRead},
};
use maka_runtime::{
    event::{Fact, ToolOutcome},
    tool_output::ToolOutput,
};

pub(super) async fn verify(log: &EventLog, expected: usize) {
    let prefix = log.prefix(2000, 8 * 1024 * 1024).await.unwrap();
    let mut metered = Vec::new();
    for row in &prefix.events {
        if row.event.invocation.session_id == "js-session"
            && matches!(
                row.event.fact,
                Fact::ToolSettled {
                    outcome: ToolOutcome::Succeeded { .. },
                    ..
                }
            )
        {
            let output = log
                .resolve_tool_result("js-session", &row.event.id)
                .await
                .unwrap();
            if let ToolOutput::Model(result) = output {
                assert_eq!(result.text, "nested answer");
                assert_eq!(result.usage.input_tokens, Some(3));
                assert_eq!(result.usage.output_tokens, Some(5));
                metered.push(maka_runtime::tool_call::metered_usage_id(&row.event.id));
            }
        }
    }
    assert_eq!(
        metered.len(),
        expected,
        "cancelled call must not invent successful usage"
    );
    while !log
        .prepare_transcript("js-session", prefix.high_water, 32)
        .await
        .unwrap()
    {}
    let rows = log
        .transcript_headers(
            "js-session",
            &TranscriptRead {
                through: maka_presentation::watermark(prefix.high_water).unwrap(),
                position: 0,
                direction: TranscriptDirection::Newer,
                limit: 200,
            },
        )
        .await
        .unwrap();
    let mut ids = std::collections::BTreeSet::new();
    let mut projected = 0;
    for row in rows {
        let bytes = log
            .transcript_fragment("js-session", row.sequence, 0, row.total_bytes)
            .await
            .unwrap();
        let message: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let id = message["id"].as_str().unwrap().to_owned();
        assert!(
            ids.insert(id.clone()),
            "duplicate projected message identity"
        );
        if metered.contains(&id) {
            assert_eq!(message["type"], "token_usage");
            assert_eq!(message["input"], 3);
            assert_eq!(message["output"], 5);
            assert!(
                log.validate_message_identity("js-session", &id)
                    .await
                    .is_err(),
                "source message must not overwrite metered usage"
            );
            projected += 1;
        }
    }
    assert_eq!(projected, expected);
}
