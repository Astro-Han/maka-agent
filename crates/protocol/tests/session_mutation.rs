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
use maka_runtime::configuration::Patch;
use serde_json::{Value, json};

fn update(patch: Value) -> Value {
    json!({"sessionId":"s1","expectedRevision":1,"patch":patch})
}

fn projection(id: &str) -> Value {
    json!({
        "id":id,"revision":2,"workspace":{"target":{"kind":"host_path","path":"/work"},"hostCwd":"/work"},
        "createdAt":0,"activityAt":1,"name":"Chat","isFlagged":false,"isArchived":false,
        "labels":[],"labelsTruncated":false,"hasUnread":false,"status":"active","backend":"ai-sdk",
        "llmConnectionId":null,"llmConnectionSlug":"default","connectionLocked":false,"model":"model",
        "sandboxMode":"workspace-write","approvalPolicy":{"kind":"on-request"},"collaborationMode":"agent","orchestrationMode":"default"
    })
}

#[test]
fn configuration_thinking_preserves_clears_and_sets_without_relaxing_other_contracts() {
    for (patch, thinking) in [
        (json!({"sandboxMode":"workspace-write"}), Patch::Keep),
        (json!({"thinkingLevel":null}), Patch::Clear),
        (
            json!({"thinkingLevel":"high"}),
            Patch::Set(ThinkingLevel::High),
        ),
        (
            json!({"modelTarget":{"kind":"explicit","connectionId":"c1",
            "connectionSlug":"provider","model":"model"},"collaborationMode":"plan",
            "orchestrationMode":"swarm","thinkingLevel":null}),
            Patch::Clear,
        ),
    ] {
        let value = update(patch);
        let input = decode_session_configuration_update_input(&value).unwrap();
        assert_eq!(input.patch.thinking_level, thinking);
        assert_eq!(serde_json::to_value(input).unwrap(), value);
    }
    for patch in [
        json!({}),
        json!({"modelTarget":{"kind":"default"}}),
        json!({"modelTarget":{"kind":"explicit","connectionId":"bad.id",
            "connectionSlug":"provider","model":"model"}}),
        json!({"modelTarget":null}),
        json!({"sandboxMode":null}),
        json!({"collaborationMode":null}),
        json!({"orchestrationMode":null}),
        json!({"thinkingLevel":false}),
        json!({"thinkingLevel":"unknown"}),
        json!({"thinkingLevel":null,"extra":true}),
    ] {
        assert!(
            decode_session_configuration_update_input(&update(patch.clone())).is_err(),
            "{patch}"
        );
    }
    let create = json!({"sessionId":"s1","workspace":{"kind":"host_path","path":"/tmp"},
        "modelTarget":{"kind":"default"},"thinkingLevel":null});
    assert!(decode_session_create_input(&create).is_err());
    assert!(decode_session_metadata_update_input(&update(json!({"thinkingLevel":null}))).is_err());
}

#[test]
fn configuration_requires_exact_input_safe_revision_and_correlated_output() {
    let value = update(json!({"thinkingLevel":null}));
    for revision in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(9_007_199_254_740_992u64),
        Value::Null,
    ] {
        let mut invalid = value.clone();
        invalid["expectedRevision"] = revision;
        assert!(decode_session_configuration_update_input(&invalid).is_err());
    }
    for revision in [json!(1.0), json!(9_007_199_254_740_991u64)] {
        let mut valid = value.clone();
        valid["expectedRevision"] = revision;
        assert!(decode_session_configuration_update_input(&valid).is_ok());
    }
    for field in ["sessionId", "expectedRevision", "patch"] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(decode_session_configuration_update_input(&invalid).is_err());
    }
    let mut invalid = value.clone();
    invalid["extra"] = json!(true);
    assert!(decode_session_configuration_update_input(&invalid).is_err());
    let input = decode_session_configuration_update_input(&value).unwrap();
    for (output, accepted) in [
        (json!({"kind":"committed","session":projection("s1")}), true),
        (
            json!({"kind":"committed","session":projection("s2")}),
            false,
        ),
        (
            json!({"kind":"revision_conflict","expectedRevision":1,"actualRevision":2}),
            true,
        ),
        (
            json!({"kind":"revision_conflict","expectedRevision":2,"actualRevision":3}),
            false,
        ),
    ] {
        let output = decode_session_update_result(&output).unwrap();
        assert_eq!(
            assert_configuration_update_output_for_input(&input, &output).is_ok(),
            accepted
        );
    }
}

#[test]
fn metadata_preserves_absence_and_validates_boundaries_before_domain_normalization() {
    for patch in [
        json!({"name":"é".repeat(160)}),
        json!({"name":"a  b"}),
        json!({"name":"\u{0085}name"}),
        json!({"labels":[]}),
        json!({"labels":["é".repeat(64), "mode:bot"]}),
        json!({"labels":(0..32).map(|i| i.to_string()).collect::<Vec<_>>()}),
        json!({"isFlagged":false}),
    ] {
        let value = update(patch);
        let decoded = decode_session_metadata_update_input(&value).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    }
    for patch in [
        json!({}),
        json!({"name":null}),
        json!({"labels":null}),
        json!({"isFlagged":null}),
        json!({"name":""}),
        json!({"name":"é".repeat(161)}),
        json!({"name":" name"}),
        json!({"name":"\u{feff}name"}),
        json!({"name":"line\nbreak"}),
        json!({"labels":["same","same"]}),
        json!({"labels":[""]}),
        json!({"labels":["é".repeat(65)]}),
        json!({"labels":[1]}),
        json!({"labels":(0..33).map(|i| i.to_string()).collect::<Vec<_>>()}),
        json!({"isFlagged":0}),
        json!({"isFlagged":true,"extra":false}),
        json!({"titleIsManual":true}),
    ] {
        assert!(
            decode_session_metadata_update_input(&update(patch.clone())).is_err(),
            "{patch}"
        );
    }
    for revision in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(9_007_199_254_740_992u64),
        Value::Null,
    ] {
        let mut value = update(json!({"isFlagged":true}));
        value["expectedRevision"] = revision;
        assert!(decode_session_metadata_update_input(&value).is_err());
    }
    let mut value = update(json!({"isFlagged":true}));
    value["expectedRevision"] = json!(1.0);
    assert_eq!(
        decode_session_metadata_update_input(&value)
            .unwrap()
            .expected_revision,
        1
    );
    for field in ["sessionId", "expectedRevision", "patch"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(decode_session_metadata_update_input(&missing).is_err());
    }
    value["extra"] = json!(true);
    assert!(decode_session_metadata_update_input(&value).is_err());
}

#[test]
fn marker_is_exact_id_only_and_update_outputs_are_correlated() {
    let input = decode_session_metadata_update_input(&update(json!({"labels":[]}))).unwrap();
    for id in ["s1", "s2"] {
        let item = projection(id);
        let output =
            decode_session_update_result(&json!({"kind":"committed","session":item})).unwrap();
        assert_eq!(
            assert_metadata_update_output_for_input(&input, &output).is_ok(),
            id == "s1"
        );
        let marker = decode_session_read_marker_set_input(
            &json!({"sessionId":"s1","readThroughMessageId":"m_1-a"}),
        )
        .unwrap();
        assert_eq!(
            assert_read_marker_output_for_input(
                &marker,
                &decode_session_catalog_projection(&item).unwrap()
            )
            .is_ok(),
            id == "s1"
        );
    }
    for expected in [1, 2] {
        let output = decode_session_update_result(
            &json!({"kind":"revision_conflict","expectedRevision":expected,"actualRevision":2}),
        )
        .unwrap();
        assert_eq!(
            assert_metadata_update_output_for_input(&input, &output).is_ok(),
            expected == 1
        );
    }
    for value in [
        json!({"kind":"committed"}),
        json!({"kind":"committed","session":null}),
        json!({"kind":"revision_conflict","expectedRevision":0,"actualRevision":2}),
        json!({"kind":"revision_conflict","expectedRevision":1,"actualRevision":0}),
        json!({"kind":"revision_conflict","expectedRevision":1,"actualRevision":2,"session":null}),
    ] {
        assert!(decode_session_update_result(&value).is_err(), "{value}");
    }
    for id in [
        json!(""),
        json!("bad.id"),
        json!("x".repeat(129)),
        json!(null),
        json!(2),
    ] {
        for field in ["sessionId", "readThroughMessageId"] {
            let mut value = json!({"sessionId":"s1","readThroughMessageId":"m1"});
            value[field] = id.clone();
            assert!(
                decode_session_read_marker_set_input(&value).is_err(),
                "{value}"
            );
        }
    }
    for value in [
        json!({"sessionId":"s1"}),
        json!({"readThroughMessageId":"m1"}),
        json!({"sessionId":"s1","readThroughMessageId":"m1","expectedRevision":1}),
    ] {
        assert!(decode_session_read_marker_set_input(&value).is_err());
    }
}
