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

use super::super::support::attachment_client as support;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use maka_client::Client;
use maka_protocol::{
    Operation, OperationErrorCode as Code,
    artifact::{ArtifactDeleteInput, ArtifactQueryInput, ArtifactQueryResult},
};
use maka_runtime::artifact::upload_artifact_id;
use serde_json::{Value, json};
use std::path::Path;

const TEXT: &str = "uploaded text 😀, outside workspace authority";
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aWQAAAABJRU5ErkJggg==";
pub(super) struct Saved {
    turn: Value,
    terminal: Value,
    rows: Vec<Value>,
}
pub(super) async fn verify(
    client: &Client,
    workspace: &Path,
    reopened: bool,
    saved: &mut Vec<Saved>,
) {
    if reopened {
        for item in saved {
            assert_eq!(
                support::rows(client, item.turn["sessionId"].as_str().unwrap()).await,
                item.rows
            );
            assert_eq!(
                client
                    .request(Operation::TurnStart, item.turn.clone())
                    .await
                    .unwrap()["turn"],
                item.terminal
            );
            for attachment in item.turn["content"]["attachments"].as_array().unwrap() {
                let ArtifactQueryResult::Artifact { artifact, .. } = client
                    .query_artifact(ArtifactQueryInput::Get {
                        session_id: item.turn["sessionId"].as_str().unwrap().into(),
                        artifact_id: attachment["ref"]["relativePath"].as_str().unwrap().into(),
                    })
                    .await
                    .unwrap()
                else {
                    panic!()
                };
                assert!(artifact.is_none());
            }
        }
        return;
    }
    let model = support::Model::start(6, |index, input| {
        let vision = index >= 3;
        let step = index % 3 + 1;
        assert_eq!(input["model"], if vision { "gpt-4o" } else { "text-model" });
        let session = if vision {
            "vision-session"
        } else {
            "text-session"
        };
        let ids = [
            upload_artifact_id(session, "text"),
            upload_artifact_id(session, "image"),
        ];
        let messages = input["messages"].as_array().unwrap();
        let serialized = serde_json::to_string(messages).unwrap();
        for id in &ids {
            assert!(serialized.contains(&format!("maka://runtime/attachments/{id}")));
        }
        if vision {
            assert_eq!(
                serialized
                    .matches(&format!("data:image/png;base64,{PNG}"))
                    .count(),
                if step == 3 { 2 } else { 1 }
            );
        } else {
            assert!(!serialized.contains(PNG));
            assert!(!serialized.contains("image_url"));
            let user = messages.iter().find(|m| m["role"] == "user").unwrap()["content"]
                .as_str()
                .unwrap();
            assert!(user.contains("consume uploaded resources"));
            assert!(user.contains("note.txt") && user.contains("pixel.png"));
        }
        let tools: Vec<_> = messages.iter().filter(|m| m["role"] == "tool").collect();
        if step == 2 {
            assert_eq!(
                serde_json::from_str::<Value>(tools.last().unwrap()["content"].as_str().unwrap())
                    .unwrap(),
                json!({"content":TEXT,"offset":0,"returnedLines":1,"totalLines":1,"next":null})
            );
        }
        if step == 3 && vision {
            assert!(
                tools
                    .iter()
                    .all(|m| !m["content"].to_string().contains(PNG))
            );
            let tool_index = messages.iter().rposition(|m| m["role"] == "tool").unwrap();
            assert!(messages[tool_index + 1..].iter().any(|m| {
                m["role"] == "user"
                    && m["content"]
                        .as_array()
                        .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"))
            }));
        }
        if step < 3 {
            support::read(&format!("read-{step}"), &ids[step - 1])
        } else {
            json!({"content":"attachment complete"})
        }
    })
    .await;
    let connection = support::configure(
        client,
        &model.url,
        json!({
            "text-model":{"vision":false},"gpt-4o":{"vision":true,"codeMode":false}
        }),
    )
    .await;
    for (session, model_id) in [("text-session", "text-model"), ("vision-session", "gpt-4o")] {
        support::session(client, &connection, workspace, session, model_id).await;
        let attachments = vec![
            support::upload(
                client,
                session,
                "text",
                "note.txt",
                "text/plain",
                TEXT.as_bytes(),
            )
            .await,
            support::upload(
                client,
                session,
                "image",
                "pixel.png",
                "image/png",
                &STANDARD.decode(PNG).unwrap(),
            )
            .await,
        ];
        let attachments = serde_json::to_value(attachments).unwrap();
        let turn = json!({"sessionId":session,"turnId":"consume","maxSteps":4,
            "content":{"text":"consume uploaded resources","attachments":attachments}});
        for (index, field, value) in [
            (0, "name", json!("forged.txt")),
            (0, "bytes", json!(TEXT.len() + 1)),
            (0, "mimeType", json!("application/json")),
            (1, "kind", json!("other")),
            (
                0,
                "ref",
                json!({"kind":"session_file","sessionId":"other-session","relativePath":attachments[0]["ref"]["relativePath"]}),
            ),
        ] {
            let mut corrupt = attachments[index].clone();
            corrupt[field] = value;
            support::rejected(client.request(Operation::TurnStart,json!({
                "sessionId":session,"turnId":"rejected","maxSteps":4,"content":{"text":"consume uploaded resources","attachments":[corrupt]}
            })).await,Code::OperationConflict);
            support::rejected(
                client
                    .request(
                        Operation::TurnQuery,
                        json!({"sessionId":session,"turnId":"rejected"}),
                    )
                    .await,
                Code::NotFound,
            );
        }
        let terminal = support::completed(client, &turn).await;
        let rows = support::rows(client, session).await;
        assert_eq!(
            rows.iter().find(|r| r["type"] == "user").unwrap()["attachments"],
            attachments
        );
        let results: Vec<_> = rows.iter().filter(|r| r["type"] == "tool_result").collect();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r["isError"] != true));
        assert_eq!(results[0]["content"], json!({"kind":"text","text":TEXT}));
        assert_eq!(
            results[1]["content"],
            json!({"kind":"image","mimeType":"image/png","ref":attachments[1]["ref"]})
        );
        assert_eq!(results[1]["content"]["ref"]["sessionId"], session);
        for attachment in attachments.as_array().unwrap() {
            client
                .delete_artifact(ArtifactDeleteInput {
                    session_id: session.into(),
                    artifact_id: attachment["ref"]["relativePath"].as_str().unwrap().into(),
                })
                .await
                .unwrap();
        }
        assert_eq!(
            client
                .request(Operation::TurnStart, turn.clone())
                .await
                .unwrap()["turn"],
            terminal
        );
        saved.push(Saved {
            turn,
            terminal,
            rows,
        });
    }
    model.finish().await;
}
