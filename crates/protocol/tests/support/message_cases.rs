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

use maka_protocol::Operation;
use serde_json::{Value, json};

pub fn inputs(add: &mut impl FnMut(Operation, &str, Value)) {
    use Operation::*;
    let submit = json!({"originHostEpoch":"epoch","sessionId":"session","messageId":"message",
        "content":{"text":"hello😀","displayText":"hello😀","attachments":[]},"placement":"current_turn"});
    let query = json!({"sessionId":"session","messageIds":["message"]});
    let entries = vec![
        (TurnMessageSubmit, submit.clone()),
        (TurnMessageQuery, query.clone()),
        (TurnMessageExecutionQuery, query.clone()),
        (
            QueueRetract,
            json!({"originHostEpoch":"epoch","sessionId":"session","retractId":"r"}),
        ),
        (
            QueueEntryRetract,
            json!({"originHostEpoch":"epoch","sessionId":"session","retractId":"r","entryId":"e"}),
        ),
        (
            QueueEntryPromote,
            json!({"originHostEpoch":"epoch","sessionId":"session","promoteId":"p","entryId":"e"}),
        ),
        (
            QueueEntryUpdate,
            json!({"originHostEpoch":"epoch","sessionId":"session","updateId":"u","entryId":"e","expectedQueueRevision":1.0,"text":" updated "}),
        ),
        (
            QueueEntriesReorder,
            json!({"originHostEpoch":"epoch","sessionId":"session","reorderId":"o","entryIds":["e"]}),
        ),
        (
            TurnInterrupt,
            json!({"originHostEpoch":"epoch","sessionId":"session","interruptId":"i","turnId":"t","runId":"r"}),
        ),
    ];
    for (op, input) in &entries {
        add(*op, "input", input.clone());
        for key in input.as_object().unwrap().keys() {
            for value in [None, Some(Value::Null), Some(json!(true))] {
                let mut bad = input.clone();
                if let Some(value) = value {
                    bad[key] = value;
                } else {
                    bad.as_object_mut().unwrap().remove(key);
                }
                add(*op, "input", bad);
            }
        }
        let mut extra = input.clone();
        extra["extra"] = json!(false);
        add(*op, "input", extra);
    }
    for (field, values) in [
        (
            "originHostEpoch",
            vec![json!(""), json!("😀".repeat(64)), json!("😀".repeat(65))],
        ),
        (
            "messageId",
            vec![json!("a.b"), json!("a".repeat(128)), json!("a".repeat(129))],
        ),
        (
            "skillIds",
            vec![
                json!([]),
                json!(["pkg:skill"]),
                json!(["bad/skill"]),
                Value::Null,
                json!(vec!["x"; 51]),
            ],
        ),
        (
            "turnOrchestration",
            vec![
                json!({"mode":"graph","source":"host_api"}),
                json!({"mode":"swarm","source":"slash_command"}),
                Value::Null,
            ],
        ),
        ("placement", vec![json!("next_turn"), json!("in_flight")]),
        (
            "content",
            vec![
                json!({"text":""}),
                json!({"text":"\u{feff}"}),
                json!({"text":"x".repeat(48*1024)}),
                json!({"text":"x".repeat(48*1024+1)}),
            ],
        ),
    ] {
        for value in values {
            let mut input = submit.clone();
            input[field] = value;
            add(TurnMessageSubmit, "input", input.clone());
            input["placement"] = json!("next_turn");
            add(TurnMessageSubmit, "input", input);
        }
    }
    for ids in [
        json!([]),
        json!(["m", "m"]),
        json!((0..64).map(|n| format!("m{n}")).collect::<Vec<_>>()),
        json!(vec!["m"; 65]),
    ] {
        let mut input = query.clone();
        input["messageIds"] = ids.clone();
        add(TurnMessageQuery, "input", input);
        add(
            TurnMessageQuery,
            "output",
            json!({"cancelledMessageIds":ids}),
        );
    }
    for text in ["", " \t\r\n", "\u{feff}", "\u{85}", " keep spaces "] {
        let mut input = entries[6].1.clone();
        input["text"] = json!(text);
        add(QueueEntryUpdate, "input", input);
    }
    for revision in [
        json!(0),
        json!(1e2),
        json!(9_007_199_254_740_991u64),
        json!(9_007_199_254_740_992u64),
        json!(-1),
        json!(0.5),
        Value::Null,
    ] {
        let mut input = entries[6].1.clone();
        input["expectedQueueRevision"] = revision.clone();
        add(QueueEntryUpdate, "input", input);
        add(
            QueueEntryUpdate,
            "output",
            json!({"queueRevision":revision}),
        );
    }
}
