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
use maka_runtime::{event::Fact, message::MessageDisposition};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_submits_one_canonical_root_and_replays_after_cancel_and_reopen() {
    let fixture = ClientFixture::new("maka-message-submit-");
    let home = fixture.workspace.join("skill-home");
    let library = home.join(".maka/skill-sources/library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(
        library.join("SKILL.md"),
        "---\nname: Library\ndescription: managed source\ncategory: custom category\n---\nNot installed.",
    )
    .unwrap();
    std::fs::create_dir_all(fixture.owner().canonical_path().join("skills/computer-use")).unwrap();
    let archival = fixture.workspace.join(".maka/skills/archival");
    std::fs::create_dir_all(&archival).unwrap();
    std::fs::write(
        archival.join("SKILL.md"),
        format!(
            "---\nname: Archival\ndescription: {}\n---\n{}",
            "archival recovery ".repeat(10_000),
            (0..400)
                .map(|n| format!("Frozen archival line {n:04} with readable instructions.\n"))
                .collect::<String>()
        ),
    )
    .unwrap();
    let legacy = fixture.workspace.join(".maka/skills/legacy");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(
        legacy.join("SKILL.md"),
        "---\nname: Legacy\ndescription: legacy entry\n---\nFrozen legacy instructions.",
    )
    .unwrap();
    for id in ["review", "disabled"] {
        let directory = fixture.workspace.join(".maka/skills").join(id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("SKILL.md"),
            "---\nname: Review\ndescription: review code\n---\nFrozen review instructions.",
        )
        .unwrap();
    }
    for id in ["tools", "write"] {
        let directory = fixture.workspace.join(".maka/skills").join(id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("SKILL.md"), format!(
            "---\nname: {}\ndescription: work\nrequired-tools: Bash\n---\nFrozen tool instructions.",
            if id == "tools" { "Tools" } else { "Write" }
        )).unwrap();
    }
    {
        let store = maka_config::ConfigurationStore::for_root(std::sync::Arc::new(fixture.owner()))
            .await
            .unwrap();
        store
            .set_skill_preference(
                0,
                "project:maka:disabled".into(),
                maka_runtime::skills::SkillPreference {
                    enabled: false,
                    pinned: false,
                },
            )
            .await
            .unwrap();
        store.close().await.unwrap();
    }
    fixture
        .run_with_options(
            "--message-submit-workspace",
            false,
            "message-submit-passed",
            maka_runtime_host::server::HostOptions {
                skill_home: Some(home.clone()),
                ..Default::default()
            },
        )
        .await;
    let log = fixture.log().await;
    let prefix = log.prefix(1000, 8 * 1024 * 1024).await.unwrap();
    assert_eq!(
        prefix
            .events
            .iter()
            .filter(|event| matches!(event.event.fact, Fact::InvocationOpened { .. }))
            .count(),
        5
    );
    let legacy = prefix
        .events
        .iter()
        .find(|event| event.event.invocation.turn_id == "legacy-skills")
        .unwrap();
    let operation = prefix
        .events
        .iter()
        .find_map(|event| match &event.event.fact {
            Fact::ToolDispatched {
                operation_id, name, ..
            } if name == "Skill" => Some(operation_id),
            _ => None,
        })
        .unwrap();
    let settled = prefix.events.iter().find(|event| matches!(&event.event.fact, Fact::ToolSettled { operation_id, .. } if operation_id == operation)).unwrap();
    let raw = log
        .resolve_tool_result("message-submit", &settled.event.id)
        .await
        .unwrap();
    let maka_runtime::tool_output::ToolOutput::Json(value) = &raw else {
        panic!("missing typed skill result")
    };
    assert_eq!(value["status"], "loaded");
    assert_eq!(value["receipt"]["invocation"], "model_tool");
    assert_eq!(value["receipt"]["ref"], "project:maka:archival");
    assert_eq!(
        value["skill"]["instructions"]
            .as_str()
            .unwrap()
            .lines()
            .count(),
        400
    );
    assert_eq!(value["skill"]["metadataTruncated"], true);
    let Fact::InvocationOpened {
        input:
            maka_runtime::input::InvocationInput::Message {
                source_messages,
                skill_invocation,
                ..
            },
        ..
    } = &legacy.event.fact
    else {
        panic!("missing legacy opening")
    };
    assert!(
        source_messages.is_empty(),
        "legacy admission must not fabricate a Message identity"
    );
    assert_eq!(skill_invocation.as_ref().unwrap().loaded[0].id, "legacy");
    let saved: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.workspace.join("message-submit.json")).unwrap(),
    )
    .unwrap();
    let user = saved["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["turnId"] == "legacy-skills" && row["type"] == "user")
        .unwrap();
    assert_eq!(user["id"], legacy.event.id);
    assert_eq!(user["displayText"], "legacy input /skill:legacy");
    for id in ["original-user-id", "cancelled-user-id"] {
        let proof = log
            .root_message("message-submit", id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(proof.source().message.message_id, id);
        assert_eq!(proof.source().disposition, MessageDisposition::TurnStarted);
        assert_ne!(proof.opening().event.invocation.turn_id, id);
        assert_eq!(
            proof.source().message.submitted_content_digest,
            if id == "original-user-id" {
                maka_runtime::input::MessageInput {
                    text: "model input 😀 /skill:review".into(),
                    display_text: Some("visible input 😀 /skill:review".into()),
                    inline_references: Some(Vec::new()),
                    .."".into()
                }
                .content_digest()
                .unwrap()
            } else {
                proof.source().message.content.content_digest().unwrap()
            }
        );
    }
    for (id, disposition, text, skill, body) in [
        (
            "late-steering-user-id",
            MessageDisposition::Steering,
            "late steering input",
            "review",
            "Frozen review instructions.",
        ),
        (
            "followup-user-id",
            MessageDisposition::Followup,
            "edited next input",
            "tools",
            "Frozen tool instructions.",
        ),
    ] {
        let proof = log
            .root_message("message-submit", id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(proof.source().disposition, disposition);
        assert_eq!(
            proof.source().message.content.display_text.as_deref(),
            Some(format!("{text} /skill:{skill}").as_str())
        );
        assert!(proof.source().message.content.text.contains(body));
        assert!(
            !proof
                .source()
                .message
                .content
                .text
                .contains("Changed after admission.")
        );
        assert_eq!(proof.source().skill_invocation.loaded[0].id, skill);
    }
    assert!(
        log.message_cancelled("message-submit", "retracted-user-id")
            .await
            .unwrap()
    );
    assert!(
        log.pending_messages("message-submit")
            .await
            .unwrap()
            .is_empty()
    );
    let bytes = serde_json::to_vec(&prefix).unwrap();
    for event in &prefix.events {
        if let Fact::InvocationOpened { configuration, .. } = &event.event.fact {
            let configuration = configuration
                .as_ref()
                .expect("message opening freezes workspace identity");
            assert_eq!(
                configuration.workspace_identity.as_ref(),
                Some(
                    &maka_fs_tools::workspace::read_identity(std::path::Path::new(
                        &configuration.cwd
                    ))
                    .unwrap()
                )
            );
        }
    }
    log.close().await.unwrap();
    fixture
        .run_with_options(
            "--message-submit-workspace",
            true,
            "message-submit-reopened",
            maka_runtime_host::server::HostOptions {
                skill_home: Some(home),
                ..Default::default()
            },
        )
        .await;
    let log = fixture.log().await;
    assert_eq!(
        serde_json::to_vec(&log.prefix(1000, 8 * 1024 * 1024).await.unwrap()).unwrap(),
        bytes
    );
    assert_eq!(
        log.resolve_tool_result("message-submit", &settled.event.id)
            .await
            .unwrap(),
        raw
    );
    log.close().await.unwrap();
}
