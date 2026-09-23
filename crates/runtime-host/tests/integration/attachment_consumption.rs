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

#![cfg(unix)]
use super::support::attachment_client::NativeHost;
use maka_event_log::{
    EventLog,
    root::{ROOT_DATABASE, RootNamespaces, RootOwner},
};
use maka_runtime::{
    event::{Fact, InvocationOutcome, ToolOutcome},
    input::InvocationInput,
    tool_output::ToolOutput,
};
use std::{os::unix::fs::PermissionsExt, time::Duration};
mod workflow;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_client_consumes_uploads_with_selected_vision_and_reopens_exact_facts() {
    let directory = tempfile::Builder::new()
        .prefix("maka-attachments-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let ns = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let root = directory.path().join("root");
    let owner = RootOwner::create(&root, &ns).unwrap();
    drop(owner);
    let mut original = None;
    let mut saved = Vec::new();
    for reopened in [false, true] {
        let native = NativeHost::open(
            RootOwner::open(&root, &ns).unwrap(),
            &directory.path().join("h.sock"),
        )
        .await;
        tokio::time::timeout(
            Duration::from_secs(30),
            workflow::verify(&native.client, directory.path(), reopened, &mut saved),
        )
        .await
        .unwrap();
        native.close().await;
        let log = EventLog::open(&root.join(ROOT_DATABASE)).await.unwrap();
        let prefix = log.prefix(200, 2 * 1024 * 1024).await.unwrap();
        let mut opened = 0;
        let mut dispatched = 0;
        let mut text_results = 0;
        let mut image_results = 0;
        let mut completed = 0;
        for stored in &prefix.events {
            match &stored.event.fact {
                Fact::InvocationOpened {
                    input: InvocationInput::Message { content, .. },
                    ..
                } => {
                    opened += 1;
                    assert_eq!(content.text, "consume uploaded resources");
                    let refs = content.attachments.as_ref().unwrap();
                    assert_eq!(refs.len(), 2);
                    assert_eq!(refs[0].name, "note.txt");
                    assert_eq!(refs[1].name, "pixel.png");
                }
                Fact::ToolDispatched { name, input, .. } => {
                    dispatched += 1;
                    assert_eq!(name, "Read");
                    assert!(
                        input["path"]
                            .as_str()
                            .unwrap()
                            .starts_with("maka://runtime/attachments/")
                    );
                    assert!(input.get("ref").is_none());
                }
                Fact::ToolSettled {
                    outcome: ToolOutcome::Succeeded { .. },
                    ..
                } => match log
                    .resolve_tool_result(&stored.event.invocation.session_id, &stored.event.id)
                    .await
                    .unwrap()
                {
                    ToolOutput::Text(value) => {
                        text_results += 1;
                        assert_eq!(value, "uploaded text 😀, outside workspace authority");
                    }
                    ToolOutput::Image(image) => {
                        image_results += 1;
                        assert_eq!(image.mime_type, "image/png");
                        assert_eq!(
                            image.reference,
                            maka_runtime::attachment::StorageRef::SessionFile {
                                session_id: stored.event.invocation.session_id.clone(),
                                relative_path: maka_runtime::artifact::upload_artifact_id(
                                    &stored.event.invocation.session_id,
                                    "image"
                                ),
                            }
                        );
                    }
                    other => panic!("unexpected attachment result: {other:?}"),
                },
                Fact::ToolSettled { outcome, .. } => panic!("attachment Read failed: {outcome:?}"),
                Fact::InvocationEnded { outcome } => {
                    completed += 1;
                    assert_eq!(outcome, &InvocationOutcome::Completed);
                }
                _ => {}
            }
        }
        assert_eq!(
            (opened, dispatched, text_results, image_results, completed),
            (2, 4, 2, 2, 2)
        );
        let bytes = serde_json::to_vec(&prefix).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains("iVBORw0KGgo"),
            "provider image bytes must not leak into canonical SQLx events"
        );
        if let Some(original) = &original {
            assert_eq!(
                &bytes, original,
                "reopen and exactretry preserve canonical prefix"
            );
        } else {
            original = Some(bytes);
        }
        log.close().await.unwrap();
    }
}
