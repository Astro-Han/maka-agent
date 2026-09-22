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

use maka_presentation::{InvocationView, tool_message_id};
use maka_runtime::event::{EventWrite, Fact, InvocationOutcome, StoredEvent, ToolOutcome};
use maka_runtime::tool_call::{ToolCallIdentity, ToolOrigin, ToolRejection};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
#[path = "tools/fixtures.rs"]
mod fixtures;
use fixtures::{opening, step, stored};
#[path = "tools/payloads.rs"]
mod payloads;
#[test]
fn accepted_tools_replay_with_distinct_ids_parentage_rejections_and_original_client_contract() {
    let mut facts = vec![opening()];
    step(&mut facts, "first", true, false);
    facts.push(Fact::ToolDispatched {
        operation_id: "first:raw".into(),
        call: ToolCallIdentity::provider("first".into(), "raw".into()),
        name: "exec".into(),
        input: json!({"code":"Read()"}),
    });
    facts.push(Fact::ToolDispatched {
        operation_id: "child".into(),
        call: ToolCallIdentity {
            tool_call_id: "nested".into(),
            origin: ToolOrigin::CodeMode {
                parent_operation_id: "first:raw".into(),
                parent_tool_call_id: "raw".into(),
            },
        },
        name: "Read".into(),
        input: json!({"path":"file"}),
    });
    facts.push(Fact::ToolSettled {
        operation_id: "child".into(),
        outcome: success(&json!({"content":"file text"}).into()),
    });
    facts.push(Fact::ToolSettled {
        operation_id: "first:raw".into(),
        outcome: success(&json!({"status":"completed"}).into()),
    });
    step(&mut facts, "second", false, false);
    facts.push(Fact::ToolRejected {
        operation_id: "second:raw".into(),
        call: ToolCallIdentity::provider("second".into(), "raw".into()),
        name: "exec".into(),
        input: json!({"code":"Read()"}),
        reason: ToolRejection::Unavailable,
    });
    step(&mut facts, "native", false, true);
    step(&mut facts, "unknown", false, false);
    facts.push(Fact::ToolDispatched {
        operation_id: "unknown:raw".into(),
        call: ToolCallIdentity::provider("unknown".into(), "raw".into()),
        name: "exec".into(),
        input: json!({"code":"Read()"}),
    });
    facts.push(Fact::InvocationEnded {
        outcome: InvocationOutcome::Failed {
            class: "unknown_outcome".into(),
            message: None,
        },
    });
    let events = stored(facts);
    let mut view = InvocationView::new(16_384).unwrap();
    let mut replay = InvocationView::new(16_384).unwrap();
    let mut rows = Vec::new();
    for stored in &events {
        let raw = match &stored.event.fact {
            Fact::ToolSettled {
                operation_id,
                outcome: ToolOutcome::Succeeded { .. },
            } => Some(
                if operation_id == "child" {
                    json!({"content":"file text"})
                } else {
                    json!({"status":"completed"})
                }
                .into(),
            ),
            _ => None,
        };
        let next = view.push_with_tool_output(stored, raw.as_ref()).unwrap();
        let roundtrip = StoredEvent {
            sequence: stored.sequence,
            event: serde_json::from_value(serde_json::to_value(&stored.event).unwrap()).unwrap(),
        };
        assert_eq!(
            next,
            replay
                .push_with_tool_output(&roundtrip, raw.as_ref())
                .unwrap()
        );
        if matches!(stored.event.fact, Fact::ModelObserved { .. }) {
            assert!(next.is_empty());
            assert!(
                view.overlay()
                    .iter()
                    .all(|message| serde_json::to_value(message).unwrap()["type"] == "assistant")
            );
        }
        rows.extend(next);
    }
    assert!(
        rows.windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    let messages: Vec<Value> = rows
        .iter()
        .map(|row| serde_json::to_value(&row.message).unwrap())
        .collect();
    let calls: Vec<_> = messages
        .iter()
        .filter(|message| message["type"] == "tool_call")
        .collect();
    assert_eq!(
        calls.len(),
        5,
        "dispatch never duplicates the accepted call"
    );
    assert_eq!(calls[0]["id"], tool_message_id("invocation", "first:raw"));
    assert_eq!(
        calls[0]["stepId"],
        messages
            .iter()
            .find(|row| row["type"] == "assistant")
            .unwrap()["id"]
    );
    assert_eq!(calls[0]["modelVisibility"], "visible");
    assert_eq!(calls[1]["parentToolCallId"], calls[0]["id"]);
    assert_eq!(calls[1]["parentOperationId"], "first:raw");
    assert_eq!(calls[1]["modelVisibility"], "hidden");
    assert_ne!(calls[0]["id"], calls[2]["id"]);
    assert!(
        calls[2].get("stepId").is_none(),
        "pure tool step has no empty assistant anchor"
    );
    let results: Vec<_> = messages
        .iter()
        .filter(|message| message["type"] == "tool_result")
        .collect();
    assert_eq!(results.len(), 4, "unknown outcome produces no result");
    assert_eq!(results[0]["toolUseId"], calls[1]["id"]);
    assert_eq!(
        results[0]["content"],
        json!({"kind":"json","value":{"content":"file text"}})
    );
    assert_eq!(results[2]["toolUseId"], calls[2]["id"]);
    assert_eq!(results[2]["isError"], true);
    assert_eq!(results[2]["content"]["kind"], "text");
    for result in &results[..3] {
        let fact = events
            .iter()
            .find(|stored| stored.event.id == result["id"])
            .unwrap();
        assert_eq!(result["ts"], 999 + fact.sequence);
    }
    assert_eq!(results[3]["toolUseId"], calls[3]["id"]);
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/support/source.mjs"))
        .arg("crates/presentation/tests/fixtures/messages.mjs")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&messages).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn success(output: &maka_runtime::tool_output::ToolOutput) -> ToolOutcome {
    let template = stored(vec![opening()]).remove(0).event;
    let write = EventWrite::tool_success(
        template.id,
        template.recorded_at,
        template.invocation,
        "operation".into(),
        output.clone().into(),
    )
    .unwrap()
    .0;
    match &write.event().fact {
        Fact::ToolSettled { outcome, .. } => outcome.clone(),
        _ => unreachable!(),
    }
}
