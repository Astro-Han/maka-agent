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

use super::*;
use maka_presentation::ProjectionError;
use maka_runtime::tool_output::ToolOutput;

fn boundary(output: &ToolOutput) -> (InvocationView, StoredEvent) {
    let mut facts = vec![opening()];
    step(&mut facts, "first", false, false);
    facts.push(Fact::ToolDispatched {
        operation_id: "first:raw".into(),
        call: ToolCallIdentity::provider("first".into(), "raw".into()),
        name: "exec".into(),
        input: json!({"code":"Read()"}),
    });
    facts.push(Fact::ToolSettled {
        operation_id: "first:raw".into(),
        outcome: success(output),
    });
    let mut events = stored(facts);
    let result = events.pop().unwrap();
    let mut view = InvocationView::new(1024).unwrap();
    for event in events {
        view.push(&event).unwrap();
    }
    (view, result)
}

#[test]
fn successful_boundary_requires_raw_and_other_facts_reject_it() {
    let raw = ToolOutput::Text("original".into());
    let (mut view, event) = boundary(&raw);
    assert!(matches!(
        view.push(&event),
        Err(ProjectionError::Invalid(_))
    ));
    assert!(
        view.push_with_tool_output(&event, Some(&raw)).is_err(),
        "invalid input poisons the projection"
    );
    let mut view = InvocationView::new(1024).unwrap();
    let opening = stored(vec![opening()]).remove(0);
    assert!(matches!(
        view.push_with_tool_output(&opening, Some(&raw)),
        Err(ProjectionError::Invalid(_))
    ));
}

#[test]
fn raw_results_have_a_separate_budget_including_complete_row_metadata() {
    let raw = ToolOutput::Text(String::new());
    let (mut view, event) = boundary(&raw);
    let row = view
        .push_with_tool_output(&event, Some(&raw))
        .unwrap()
        .remove(0);
    let overhead = serde_json::to_vec(&row.message).unwrap().len();
    let limit = 16 * 1024 * 1024;
    for (extra, succeeds) in [(0, true), (1, false)] {
        let raw = ToolOutput::Text("x".repeat(limit - overhead + extra));
        let (mut view, event) = boundary(&raw);
        let result = view.push_with_tool_output(&event, Some(&raw));
        if succeeds {
            let message = &result.unwrap().remove(0).message;
            assert_eq!(serde_json::to_vec(message).unwrap().len(), limit);
        } else {
            assert!(matches!(result, Err(ProjectionError::TooLarge)));
        }
    }
}
