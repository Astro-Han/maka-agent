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

use maka_agent::project_model_history;
use maka_event_log::EventLog;
use maka_runtime::{
    event::{Fact, Invocation, RuntimeEvent},
    input::{DeliveredMessage, InvocationInput, MessageInput},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[tokio::test]
async fn attachment_references_remain_structured_and_match_the_original_model_formatter_after_reopen()
 {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let attachments = [
        json!([{"kind":"image","name":"😀 <image> \"name\"","mimeType":"image/png","bytes":8,
            "ref":{"kind":"session_file","sessionId":"session0","relativePath":"image-id"}},
            {"kind":"pdf","name":"report.pdf","mimeType":"application/pdf","bytes":100,
            "ref":{"kind":"session_file","sessionId":"session0","relativePath":"pdf-id"}}]),
        json!([{"kind":"code","name":"source.rs","mimeType":"text/plain","bytes":10,
            "ref":{"kind":"workspace_file","relativePath":"src/file.rs"}},
            {"kind":"other","name":"external","mimeType":"text/plain","bytes":1,
            "ref":{"kind":"external_file","absolutePath":"/tmp/file"}},
            {"kind":"other","name":"not-opaque","mimeType":"text/plain","bytes":1,
            "ref":{"kind":"session_file","sessionId":"session1","relativePath":"folder/file"}}]),
        json!([]),
    ];
    let mut cases = Vec::new();
    for (index, attachments) in attachments.into_iter().enumerate() {
        let session = format!("session{index}");
        let input: MessageInput = serde_json::from_value(json!({
            "text":"original question", "display_text":"visible chips",
            "attachments":attachments,
            "quotes":[{"text":"quoted <data>","label":"label"}],
            "directory_references":[{"hostId":"root","path":"/tmp/<folder>"}],
            "inline_references":[]
        }))
        .unwrap();
        log.append(
            &maka_runtime::event::EventWrite::plain(RuntimeEvent::new(
                Invocation {
                    session_id: session.clone(),
                    turn_id: format!("turn{index}"),
                    run_id: format!("run{index}"),
                    invocation_id: format!("invocation{index}"),
                },
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        source_messages: Vec::new(),
                        content: input.clone(),
                        request_fingerprint: None,
                    },
                },
            ))
            .unwrap(),
        )
        .await
        .unwrap();
        let steering = RuntimeEvent::new(
            Invocation {
                session_id: session.clone(),
                turn_id: format!("turn{index}"),
                run_id: format!("run{index}"),
                invocation_id: format!("invocation{index}"),
            },
            Fact::MessageSteered {
                message: Box::new(DeliveredMessage {
                    message_id: "same-client-id".into(),
                    content: input.clone(),
                    submitted_content_digest: format!("sha256:{}", "a".repeat(64)),
                }),
            },
        );
        log.append(&maka_runtime::event::EventWrite::plain(steering).unwrap())
            .await
            .unwrap();
        let prefix = log
            .scoped_prefix(
                maka_runtime::event::LogScope::Session {
                    id: session.clone(),
                },
                10,
                32 * 1024,
            )
            .await
            .unwrap();
        let model =
            serde_json::to_value(project_model_history(&prefix, &session).unwrap()).unwrap();
        let text = model[0]["content"][0]["text"].as_str().unwrap();
        let mut view = maka_presentation::InvocationView::new(32 * 1024).unwrap();
        let rows = view.push(&prefix.events[0]).unwrap();
        let user = serde_json::to_value(&rows[0].message).unwrap();
        let steered = view.push(&prefix.events[1]).unwrap();
        let steered = serde_json::to_value(&steered[0].message).unwrap();
        assert_eq!(steered["id"], "same-client-id");
        assert_eq!(steered["text"], input.text);
        assert_eq!(steered["attachments"], attachments);
        assert_eq!(
            model.as_array().unwrap().len(),
            2,
            "equal text has two distinct message identities"
        );
        assert_eq!(user["attachments"], attachments);
        assert_eq!(user["text"], input.text);
        assert_eq!(user["displayText"], input.display_text.as_deref().unwrap());
        let suffix: MessageInput = serde_json::from_value(json!({
            "text": "😀 @file", "display_text": null,
            "inline_references": [{"kind":"workspace_file","value":"@file","label":"file","start":3}]
        })).unwrap();
        let aggregate_sources = [input.clone(), suffix];
        let aggregated = maka_runtime::message::aggregate(aggregate_sources.iter());
        cases.push(
            json!({"session":session,"input":serde_json::to_value(&input).unwrap(),
            "steering":model[1]["content"][0]["text"],
            "aggregation":{"sources":aggregate_sources,"expected":aggregated},
            "content":{"text":input.text,"attachments":attachments,"quotes":input.quotes,
                "directoryReferences":input.directory_references},"text":text}),
        );
    }
    let before = serde_json::to_vec(&log.prefix(10, 128 * 1024).await.unwrap()).unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        serde_json::to_vec(&log.prefix(10, 128 * 1024).await.unwrap()).unwrap(),
        before
    );
    for case in &cases {
        let session = case["session"].as_str().unwrap();
        let prefix = log
            .scoped_prefix(
                maka_runtime::event::LogScope::Session { id: session.into() },
                10,
                32 * 1024,
            )
            .await
            .unwrap();
        let Fact::InvocationOpened {
            input: InvocationInput::Message { content, .. },
            ..
        } = &prefix.events[0].event.fact
        else {
            panic!("canonical user input");
        };
        assert_eq!(serde_json::to_value(content).unwrap(), case["input"]);
        assert_eq!(
            serde_json::to_value(project_model_history(&prefix, session).unwrap()).unwrap()[0]["content"]
                [0]["text"],
            case["text"]
        );
        assert_eq!(
            serde_json::to_value(project_model_history(&prefix, session).unwrap()).unwrap()[1]["content"]
                [0]["text"],
            case["steering"]
        );
    }
    log.close().await.unwrap();
    compare(&cases);
}
fn compare(cases: &[Value]) {
    let mut child = Command::new("node")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/client-attachment-projection.mjs"),
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
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("original-attachment-reference-projection")
    );
}
