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

use maka_protocol::{Operation, message};
use serde_json::{Value, json};
#[path = "support/message_cases.rs"]
mod cases_input;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn original_message_codec_agrees_on_identity_intent_queue_states_and_bounded_projections() {
    use Operation::*;
    let mut cases = Vec::new();
    let mut add = |operation: Operation, direction: &str, input: Value| {
        let decoded = match direction {
            "projection" => {
                message::decode_queue_projection(&input).map(|v| serde_json::to_value(v).unwrap())
            }
            "output" => {
                message::decode_output(operation, &input).map(|v| serde_json::to_value(v).unwrap())
            }
            _ => message::decode_input(operation, &input).map(|v| serde_json::to_value(v).unwrap()),
        };
        let expected = match decoded {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(_) => json!({"ok":false}),
        };
        cases.push(
            json!({"operation":operation,"direction":direction,"input":input,"expected":expected}),
        );
    };
    cases_input::inputs(&mut add);
    let preparation = json!([]);
    let entry = json!({"entryId":"e","messageId":"m","content":{"text":"hello"},"placement":"current_turn","state":"retracted"});
    let turn = json!({"sessionId":"s","turnId":"t","runId":"r","status":"completed","terminalEventId":"terminal"});
    for (op, output) in [
        (
            TurnMessageSubmit,
            json!({"disposition":"turn_started","turnId":"t","preparation":preparation}),
        ),
        (
            TurnMessageSubmit,
            json!({"disposition":"steering","queueRevision":1.0,"preparation":preparation}),
        ),
        (
            TurnMessageSubmit,
            json!({"disposition":"followup","preparation":preparation}),
        ),
        (
            TurnMessageSubmit,
            json!({"disposition":"blocked","message":"Document unavailable","preparation":[{"source":{"kind":"input","name":"review","packageId":"reviewer","entryId":"entry","activation":"1","revision":"1"},"receipt":{"missing":"report.md"}}]}),
        ),
        (TurnMessageQuery, json!({"cancelledMessageIds":["m"]})),
        (
            TurnMessageExecutionQuery,
            json!({"resolutions":[{"state":"pending","messageId":"a"},{"state":"cancelled","messageId":"b"},{"state":"owned","messageId":"c","turnId":"t","runId":"r"},{"state":"not_admitted","messageId":"d"}]}),
        ),
        (
            QueueRetract,
            json!({"queueRevision":1,"retracted":[entry.clone()]}),
        ),
        (QueueEntryRetract, json!({"queueRevision":0})),
        (QueueEntryPromote, json!({"queueRevision":1})),
        (QueueEntryUpdate, json!({"queueRevision":1})),
        (QueueEntriesReorder, json!({"queueRevision":1})),
        (
            TurnInterrupt,
            json!({"queueRevision":1,"retracted":[entry.clone()],"turn":turn}),
        ),
    ] {
        add(op, "output", output.clone());
        for key in output.as_object().unwrap().keys() {
            let mut missing = output.clone();
            missing.as_object_mut().unwrap().remove(key);
            add(op, "output", missing);
            let mut null = output.clone();
            null[key] = Value::Null;
            add(op, "output", null);
        }
        let mut extra = output;
        extra["extra"] = json!(true);
        add(op, "output", extra);
    }
    add(
        TurnMessageSubmit,
        "output",
        json!({"disposition":"blocked","message":"Document unavailable","preparation":preparation}),
    );
    for resolution in [
        json!({"state":"unknown","messageId":"m"}),
        json!({"state":"pending","messageId":"m","turnId":"t"}),
        json!({"state":"not_admitted","messageId":"m","runId":"r"}),
        json!({"state":"owned","messageId":"m","turnId":"t"}),
    ] {
        add(
            TurnMessageExecutionQuery,
            "output",
            json!({"resolutions":[resolution]}),
        );
    }
    let owned = json!({"state":"owned","messageId":"m","turnId":"t","runId":"r"});
    add(
        TurnMessageExecutionQuery,
        "output",
        json!({"resolutions":[owned.clone(),owned]}),
    );
    let mut queue = json!({"hostEpoch":"epoch","queueRevision":0,"steering":[],"followup":[]});
    add(TurnMessageSubmit, "projection", queue.clone());
    // Admission and queued/retracted snapshots share the same meaningful-content
    // rule. Inline display markers alone never constitute a model message.
    for (content, accepted) in [
        (json!({"text":"", "quotes":[{"text":"quoted only"}]}), true),
        (
            json!({"text":"", "directoryReferences":[{"hostId":"h", "path":"/workspace"}]}),
            true,
        ),
        (
            json!({"text":"", "attachments":[{"kind":"other", "name":"a", "mimeType":"text/plain", "bytes":1,
            "ref":{"kind":"workspace_file", "relativePath":"a.txt"}}]}),
            true,
        ),
        (json!({"text":" \n"}), true),
        (
            json!({"text":"", "quotes":[], "attachments":[], "directoryReferences":[]}),
            false,
        ),
        (
            json!({"text":"", "displayText":"@a", "inlineReferences":[{"kind":"workspace_file", "value":"@a", "label":"a", "start":0}]}),
            false,
        ),
    ] {
        let start = json!({"sessionId":"s", "turnId":"t", "content":content});
        assert_eq!(
            maka_protocol::turn::decode_turn_start_input(&start).is_ok(),
            accepted
        );
        add(
            TurnMessageSubmit,
            "input",
            json!({"originHostEpoch":"epoch", "sessionId":"s", "messageId":"m",
            "placement":"current_turn", "content":content}),
        );
        let mut item = entry.clone();
        item["content"] = content;
        add(
            QueueRetract,
            "output",
            json!({"queueRevision":0,"retracted":[item.clone()]}),
        );
        item["state"] = json!("queued");
        let mut projection = queue.clone();
        projection["steering"] = json!([item]);
        add(TurnMessageSubmit, "projection", projection);
    }
    for ref_id in [
        "read-image:owner-1".to_owned(),
        "😀".repeat(512),
        "😀".repeat(513),
        String::new(),
    ] {
        let mut context = entry.clone();
        context["content"]["attachments"] = json!([{
            "kind":"image", "name":"preview", "mimeType":"image/png", "bytes":42,
            "ref":{"kind":"session_context", "sessionId":"s", "refId":ref_id}
        }]);
        add(
            QueueRetract,
            "output",
            json!({"queueRevision":0,"retracted":[context.clone()]}),
        );
        context["state"] = json!("queued");
        let mut value = queue.clone();
        value["steering"] = json!([context.clone()]);
        add(TurnMessageSubmit, "projection", value);
        let content = context["content"].clone();
        add(
            TurnMessageSubmit,
            "input",
            json!({
                "originHostEpoch":"epoch", "sessionId":"s", "messageId":"m",
                "placement":"current_turn", "content":content
            }),
        );
        // The canonical snapshot grammar must never broaden the older turn.start inlet.
        let mut start = json!({"sessionId":"s", "turnId":"t", "content":content});
        assert!(maka_protocol::turn::decode_turn_start_input(&start).is_err());
        start["content"]
            .as_object_mut()
            .unwrap()
            .remove("attachments");
        assert!(maka_protocol::turn::decode_turn_start_input(&start).is_ok());
    }
    for placement in ["current_turn", "next_turn"] {
        for state in ["queued", "in_flight", "retracted"] {
            let mut item = entry.clone();
            item["placement"] = json!(placement);
            item["state"] = json!(state);
            for lane in ["steering", "followup"] {
                let mut value = queue.clone();
                value[lane] = json!([item.clone()]);
                add(TurnMessageSubmit, "projection", value);
            }
            add(
                QueueRetract,
                "output",
                json!({"queueRevision":0,"retracted":[item]}),
            );
        }
    }
    let mut item = entry;
    item["state"] = json!("queued");
    queue["steering"] = json!([item.clone()]);
    for field in ["entryId", "messageId"] {
        let mut duplicate = item.clone();
        duplicate[if field == "entryId" {
            "messageId"
        } else {
            "entryId"
        }] = json!("different");
        duplicate["placement"] = json!("next_turn");
        let mut value = queue.clone();
        value["followup"] = json!([duplicate]);
        add(TurnMessageSubmit, "projection", value);
    }
    for count in [64, 65] {
        let items: Vec<_> = (0..count)
            .map(|n| {
                let mut item = item.clone();
                item["entryId"] = json!(format!("e{n}"));
                item["messageId"] = json!(format!("m{n}"));
                item
            })
            .collect();
        let mut value = queue.clone();
        value["steering"] = json!(items);
        add(TurnMessageSubmit, "projection", value);
    }
    for len in [24 * 1024, 27 * 1024, 28 * 1024] {
        let mut a = item.clone();
        a["content"]["text"] = json!("x".repeat(len));
        let mut b = a.clone();
        b["entryId"] = json!("other");
        b["messageId"] = json!("other");
        let mut value = queue.clone();
        value["steering"] = json!([a.clone(), b.clone()]);
        add(TurnMessageSubmit, "projection", value);
        a["state"] = json!("retracted");
        b["state"] = json!("retracted");
        add(
            QueueRetract,
            "output",
            json!({"queueRevision":0,"retracted":[a,b]}),
        );
    }
    compare(&cases);
}

fn compare(cases: &[Value]) {
    let mut child = Command::new("node")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/client-message-contract.mjs"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(cases).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("original-message-admission-codec"));
}
