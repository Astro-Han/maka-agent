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

use maka_protocol::session::*;
use serde_json::{Value, json};

fn create() -> Value {
    json!({"sessionId":"s1","workspace":{"kind":"host_path","path":"/work"},"modelTarget":{"kind":"default"}})
}
fn projection() -> Value {
    json!({
        "id":"s1","revision":1,"workspace":{"target":{"kind":"host_path","path":"/work"},"hostCwd":"/work"},
        "createdAt":0,"activityAt":1,"name":"New Chat","isFlagged":false,"isArchived":false,
        "labels":[],"labelsTruncated":false,"hasUnread":false,"status":"active","backend":"ai-sdk",
        "llmConnectionId":null,"llmConnectionSlug":"default","connectionLocked":false,"model":"model",
        "sandboxMode":"workspace-write","approvalPolicy":{"kind":"on-request"},"collaborationMode":"agent","orchestrationMode":"default"
    })
}
#[test]
fn create_accepts_wire_options_without_materializing_defaults() {
    for executor in ["a", "Agent.v1_foo:bar-1", &"a".repeat(128)] {
        let value = json!({"sessionId":"s1","workspace":{"kind":"host_path","path":"/work"},"executorId":executor});
        let decoded = decode_session_create_input(&value).unwrap();
        assert!(
            matches!(&decoded.target, SessionCreateTarget::Executor { executor_id, .. } if executor_id.as_str() == executor)
        );
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
        let mut configured = value;
        configured["executorSettings"] = json!({"model":"custom/model", "thinkingLevel":"max"});
        assert_eq!(
            serde_json::to_value(decode_session_create_input(&configured).unwrap()).unwrap(),
            configured
        );
        for settings in [
            json!({"model":" "}),
            json!({"model":"x".repeat(513)}),
            json!({"model":"bad\nmodel"}),
            json!({"thinkingLevel":"automatic"}),
        ] {
            configured["executorSettings"] = settings;
            assert!(decode_session_create_input(&configured).is_err());
        }
    }
    let decoded = decode_session_create_input(&create()).unwrap();
    assert_eq!(
        decoded.thinking_level,
        SessionThinkingPreference::ModelDefault
    );
    let mut explicit_default = create();
    explicit_default["thinkingLevel"] = Value::Null;
    let explicit = decode_session_create_input(&explicit_default).unwrap();
    assert_eq!(
        explicit.thinking_level,
        SessionThinkingPreference::ProviderDefault
    );
    assert_eq!(serde_json::to_value(explicit).unwrap(), explicit_default);
    assert_eq!(decoded.name, None);
    assert_eq!(decoded.sandbox_mode, None);
    assert!(
        !serde_json::to_value(decoded)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("labels")
    );
    for path in [
        "/",
        "C:/work",
        "Z:\\work",
        "\\\\host\\share",
        "//host/share",
    ] {
        let mut value = create();
        value["workspace"]["path"] = json!(path);
        assert!(decode_session_create_input(&value).is_ok(), "{path}");
    }
    let mut value = create();
    value["workspace"] = json!({"kind":"project","projectId":"project_1"});
    value["modelTarget"] =
        json!({"kind":"explicit","connectionId":"c1","connectionSlug":" x ","model":"m"});
    value["mode"] = json!("bot");
    value["sandboxMode"] = json!("read-only");
    value["labels"] = json!(["mode:bot"]);
    assert!(decode_session_create_input(&value).is_ok());
}
#[test]
fn create_rejects_unknown_null_invalid_ids_and_invalid_text() {
    let mut missing = create();
    missing.as_object_mut().unwrap().remove("modelTarget");
    assert!(decode_session_create_input(&missing).is_err());
    for executor in [
        json!(null),
        json!(""),
        json!("1agent"),
        json!("a b"),
        json!("é"),
        json!("a/agent"),
        json!("a".repeat(129)),
    ] {
        let mut value = missing.clone();
        value["executorId"] = executor;
        assert!(decode_session_create_input(&value).is_err());
    }
    let mut both = create();
    both["executorId"] = json!("valid");
    assert!(decode_session_create_input(&both).is_err());
    let mut extra = missing;
    extra["executorId"] = json!("valid");
    extra["unknown"] = json!(1);
    assert!(decode_session_create_input(&extra).is_err());
    for field in [
        "mode",
        "name",
        "labels",
        "toolProfile",
        "sandboxMode",
        "collaborationMode",
        "orchestrationMode",
    ] {
        let mut value = create();
        value[field] = Value::Null;
        assert!(decode_session_create_input(&value).is_err(), "{field}");
    }
    let mut extra = create();
    extra["modelTarget"]["model"] = json!("unexpected");
    assert!(decode_session_create_input(&extra).is_err());
    for (key, bad) in [
        ("sessionId", json!("space id")),
        ("sessionId", json!("x".repeat(129))),
        ("name", Value::Null),
        ("name", json!(" padded")),
        ("name", json!("\u{feff}name")),
        ("name", json!("é".repeat(161))),
        ("name", json!("line\nbreak")),
        ("labels", json!(["dup", "dup"])),
        ("labels", json!(vec!["x"; 33])),
        ("mode", json!("chat")),
        ("toolProfile", json!("coding")),
        ("thinkingLevel", json!("unknown")),
        ("unexpected", json!(true)),
    ] {
        let mut value = create();
        value[key] = bad;
        assert!(
            decode_session_create_input(&value).is_err(),
            "{key}: {value}"
        );
    }
    for path in ["relative", "C:relative", "\\\\host", "\\\\host\\"] {
        let mut value = create();
        value["workspace"]["path"] = json!(path);
        assert!(decode_session_create_input(&value).is_err(), "{path}");
    }
}
#[test]
fn projection_requires_every_authoritative_field_and_preserves_nullable_connection() {
    let value = projection();
    for field in value.as_object().unwrap().keys() {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            decode_session_catalog_projection(&missing).is_err(),
            "{field}"
        );
    }
    assert!(decode_session_catalog_projection(&value).is_ok());
    for (key, bad) in [
        ("revision", json!(0)),
        ("activityAt", json!(9_007_199_254_740_992u64)),
        ("thinkingLevel", Value::Null),
        ("lastMessageAt", Value::Null),
        ("subagent", json!({"parentSessionId":"s2","agentId":null})),
        (
            "liveRunState",
            json!({"schemaVersion":2,"runningTurnIds":[]}),
        ),
        (
            "liveRunState",
            json!({"schemaVersion":1,"runningTurnIds":["t","t"]}),
        ),
        ("revisionIndex", json!(0)),
        ("backend", json!("other")),
    ] {
        let mut bad_value = value.clone();
        bad_value[key] = bad;
        assert!(
            decode_session_catalog_projection(&bad_value).is_err(),
            "{key}"
        );
    }
    let mut mismatch = value.clone();
    mismatch["workspace"]["hostCwd"] = json!("/other");
    assert!(decode_session_catalog_projection(&mismatch).is_err());
    for legacy in ["review", "done"] {
        let mut old = value.clone();
        old["status"] = json!(legacy);
        old["revision"] = json!(1.0);
        let output =
            serde_json::to_value(decode_session_catalog_projection(&old).unwrap()).unwrap();
        assert_eq!(output["status"], "active");
        assert_eq!(output["revision"], 1);
    }
}
#[test]
fn query_checks_revision_cursor_page_bounds_and_required_nullables() {
    let revision = format!("sha256:{}", "a".repeat(64));
    assert!(
        decode_session_catalog_query_input(
            &json!({"kind":"list_continue","revision":revision,"cursor":"cursor"})
        )
        .is_ok()
    );
    for value in [
        json!({"kind":"list_start","cursor":"x"}),
        json!({"kind":"get","sessionId":"bad.id"}),
        json!({"kind":"list_continue","revision":"sha256:ABC","cursor":"c"}),
        json!({"kind":"list_continue","revision":revision,"cursor":"é".repeat(257)}),
    ] {
        assert!(decode_session_catalog_query_input(&value).is_err());
    }
    assert!(decode_session_catalog_query_result(&json!({"kind":"session","session":null})).is_ok());
    assert!(decode_session_catalog_query_result(&json!({"kind":"session"})).is_err());
    let mut page = json!({"kind":"page","revision":revision,"sessions":[],"nextCursor":null});
    assert!(decode_session_catalog_query_result(&page).is_ok());
    page["sessions"] = json!(vec![projection(); 33]);
    assert!(decode_session_catalog_query_result(&page).is_err());
    let mut large = projection();
    large["lastMessagePreview"] = json!("a".repeat(4096));
    page["sessions"] = json!(vec![large; 12]);
    assert!(decode_session_catalog_query_result(&page).is_err());
}
#[test]
fn retirement_validates_revision_numbers_and_request_result_relations() {
    let create = decode_session_create_input(&create()).unwrap();
    let item = decode_session_catalog_projection(&projection()).unwrap();
    assert!(assert_create_output_for_input(&create, &item).is_ok());
    let mut other = projection();
    other["id"] = json!("s2");
    assert!(
        assert_create_output_for_input(
            &create,
            &decode_session_catalog_projection(&other).unwrap()
        )
        .is_err()
    );
    let active =
        decode_session_lifecycle_set_input(&json!({"sessionId":"s1","state":"active"})).unwrap();
    assert!(assert_lifecycle_output_for_input(&active, &item).is_ok());
    let archived = SessionLifecycleSetInput {
        state: SessionLifecycleState::Archived,
        ..active
    };
    assert!(assert_lifecycle_output_for_input(&archived, &item).is_err());
    let input =
        decode_session_remove_input(&json!({"sessionId":"s1","expectedRevision":1e0})).unwrap();
    for (value, matches) in [
        (json!({"kind":"removed","sessionId":"s1"}), true),
        (json!({"kind":"removed","sessionId":"s2"}), false),
        (
            json!({"kind":"revision_conflict","expectedRevision":1,"actualRevision":2}),
            true,
        ),
        (
            json!({"kind":"revision_conflict","expectedRevision":2,"actualRevision":2}),
            false,
        ),
    ] {
        let result = decode_session_remove_result(&value).unwrap();
        assert_eq!(
            assert_remove_output_for_input(&input, &result).is_ok(),
            matches
        );
    }
    for revision in [json!(0), json!(-1), json!(1.5), Value::Null] {
        assert!(
            decode_session_remove_input(&json!({"sessionId":"s1","expectedRevision":revision}))
                .is_err()
        );
    }
    assert!(
        decode_session_remove_result(
            &json!({"kind":"removed","sessionId":"s1","archivedSubtaskCount":null})
        )
        .is_err()
    );
    assert!(decode_session_remove_preview_result(&json!({"archivableSubtaskCount":0.0})).is_ok());
    assert!(decode_session_remove_preview_result(&json!({"archivableSubtaskCount":-1})).is_err());
}
