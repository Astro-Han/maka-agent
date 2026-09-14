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

use base64::{Engine, engine::general_purpose::STANDARD};
use maka_protocol::capability::decode_host_frame;
use maka_protocol::capability::{decode_client_frame, is_client_frame_kind, is_host_frame_kind};
use maka_runtime::capability::{AdmissionEvidence, ClientFrame, HostFrame};
use serde_json::{Value, json};

#[test]
fn closed_frame_kinds_and_exact_admission_evidence() {
    let frame = json!({"kind":"client.capability.accepted","invocationId":"i-1",
        "admissionEvidence":{"kind":"browser_url","url":"not a URL"}});
    assert_eq!(
        decode_client_frame(&frame).unwrap(),
        ClientFrame::Accepted {
            invocation_id: "i-1".into(),
            admission_evidence: AdmissionEvidence::BrowserUrl {
                url: "not a URL".into()
            },
        }
    );
    assert_eq!(
        serde_json::to_value(decode_client_frame(&frame).unwrap()).unwrap(),
        frame
    );
    for evidence in [
        json!({"kind":"none","url":"x"}),
        json!({"kind":"future"}),
        json!({"kind":"browser_url","url":""}),
    ] {
        let mut bad = frame.clone();
        bad["admissionEvidence"] = evidence;
        assert!(decode_client_frame(&bad).is_err());
    }
    for kind in [
        "call",
        "service_call",
        "cancel",
        "release",
        "registration_release",
        "admitted",
        "interaction_result",
    ] {
        let kind = format!("client.capability.{kind}");
        assert!(is_host_frame_kind(&kind));
        assert!(!is_client_frame_kind(&kind));
        assert!(decode_client_frame(&json!({"kind":kind,"invocationId":"i"})).is_err());
    }
    assert!(!is_client_frame_kind("client.capability.future"));
    assert_eq!(
        serde_json::to_value(HostFrame::RegistrationRelease {
            registration_id: "r".into()
        })
        .unwrap(),
        json!({"kind":"client.capability.registration_release","registrationId":"r"})
    );
    let call = HostFrame::Call {
        invocation_id: "i".into(),
        registration_id: "r".into(),
        offer_id: "o".into(),
        server_id: "server".into(),
        tool_name: "tool".into(),
        arguments: Default::default(),
        session_id: "s".into(),
        turn_id: "t".into(),
        tool_call_id: "tc".into(),
        cwd: None,
    };
    assert_eq!(
        serde_json::to_value(call).unwrap(),
        json!({
            "kind":"client.capability.call", "invocationId":"i", "registrationId":"r", "offerId":"o",
            "serverId":"server", "toolName":"tool", "arguments":{}, "sessionId":"s", "turnId":"t", "toolCallId":"tc"
        })
    );
}

#[test]
fn progress_and_chunk_bounds_follow_the_source_boundary() {
    for (current, total, valid) in [
        (0.0, 1.0, true),
        (1024.0, 1024.0, true),
        (0.0, 0.0, false),
        (0.0, 1025.0, false),
        (2.0, 1.0, false),
        (0.5, 1.0, false),
    ] {
        assert_eq!(
            decode_client_frame(
                &json!({"kind":"client.capability.progress", "invocationId":"i",
            "current":current,"total":total})
            )
            .is_ok(),
            valid
        );
    }
    for (bytes, chunks, valid) in [
        (1, 1, true),
        (36864, 1, true),
        (36865, 2, true),
        (25165824, 683, true),
        (0, 1, false),
        (36865, 1, false),
        (25165825, 683, false),
    ] {
        assert_eq!(
            decode_client_frame(
                &json!({"kind":"client.capability.result_start", "invocationId":"i",
            "byteLength":bytes,"chunkCount":chunks})
            )
            .is_ok(),
            valid
        );
    }
    for (data, valid) in [
        ("AA==", true),
        ("AB==", false),
        ("AA", false),
        ("", false),
        ("AA==\n", false),
    ] {
        assert_eq!(
            decode_client_frame(
                &json!({"kind":"client.capability.result_chunk", "invocationId":"i",
            "index":9007199254740991_u64,"data":data})
            )
            .is_ok(),
            valid
        );
    }
    // Index-to-transfer bounds are intentionally checked by the invocation owner.
    for size in [36864, 36865] {
        assert_eq!(
            decode_client_frame(
                &json!({"kind":"client.capability.result_chunk", "invocationId":"i",
            "index":0,"data":STANDARD.encode(vec![0;size])})
            )
            .is_ok(),
            size == 36864
        );
    }
}

#[test]
fn inline_budget_utf16_and_structured_json_are_enforced() {
    let frame = |result: Value| json!({"kind":"client.capability.result","invocationId":"i","result":result});
    let empty = json!({"content":[{"type":"text","text":""}]});
    let overhead = serde_json::to_vec(&empty).unwrap().len();
    for length in [40960 - overhead, 40961 - overhead] {
        assert_eq!(
            decode_client_frame(&frame(
                json!({"content":[{"type":"text","text":"x".repeat(length)}]})
            ))
            .is_ok(),
            length == 40960 - overhead
        );
    }
    let with_null = frame(json!({"content":[],"structuredContent":null}));
    assert_eq!(
        serde_json::to_value(decode_client_frame(&with_null).unwrap()).unwrap(),
        with_null
    );
    assert!(decode_client_frame(&frame(json!({"content":[],"structuredContent":{"":1}}))).is_err());
    for (length, valid) in [(2048, true), (2049, false)] {
        assert_eq!(
            decode_client_frame(
                &json!({"kind":"client.capability.failed","invocationId":"i",
            "message":"😀".repeat(length)})
            )
            .is_ok(),
            valid
        );
    }
    for id in ["", "i.1", "é", "with space"] {
        assert!(
            decode_client_frame(
                &json!({"kind":"client.capability.rejected","invocationId":id,"message":"failed"})
            )
            .is_err()
        );
    }
    assert!(decode_client_frame(&json!({"kind":"client.capability.failed","invocationId":"i","message":"failed","extra":0})).is_err());
}

#[test]
fn interaction_request_uses_validated_form_projection() {
    let mut frame = json!({"kind":"client.capability.interaction_request", "invocationId":"i", "interactionId":"q",
        "request":{"message":"Continue?","requester":{"name":"Provider","source":""},"fields":[]}});
    assert!(matches!(
        decode_client_frame(&frame).unwrap(),
        ClientFrame::InteractionRequest { .. }
    ));
    frame["request"]["requester"]["source"] = json!(null);
    assert!(decode_client_frame(&frame).is_err());
    frame["request"]["requester"]["source"] = json!("x".repeat(513));
    assert!(decode_client_frame(&frame).is_err());
}

#[test]
fn host_call_and_service_inputs_enforce_wire_boundaries() {
    let call = json!({"kind":"client.capability.call", "invocationId":"i", "registrationId":"r",
        "offerId":"o", "serverId":"server.with.dots", "toolName":"tool with spaces", "arguments":{},
        "sessionId":"s", "turnId":"t", "toolCallId":"tc", "cwd":"relative/path"});
    let service = json!({"kind":"client.capability.service_call", "invocationId":"i", "registrationId":"r",
        "serviceId":"s", "version":"v.1", "method":"get", "input":{}});
    for (frame, input_key) in [(&call, "arguments"), (&service, "input")] {
        assert_eq!(
            serde_json::to_value(decode_host_frame(frame).unwrap()).unwrap(),
            *frame
        );
        for (length, valid) in [(40952, true), (40953, false)] {
            let mut candidate = frame.clone();
            candidate[input_key] = json!({"x":"x".repeat(length)});
            assert_eq!(decode_host_frame(&candidate).is_ok(), valid);
        }
        for input in [json!([]), json!({"":0}), json!({"x":vec![0;8191]})] {
            let mut candidate = frame.clone();
            candidate[input_key] = input;
            assert!(decode_host_frame(&candidate).is_err());
        }
        for depth in [32, 33] {
            let mut input = json!(0);
            for _ in 0..depth {
                input = json!({"x":input});
            }
            let mut candidate = frame.clone();
            candidate[input_key] = input;
            assert_eq!(decode_host_frame(&candidate).is_ok(), depth == 32);
        }
        let mut candidate = frame.clone();
        candidate["invocationId"] = json!("i.invalid");
        assert!(decode_host_frame(&candidate).is_err());
        candidate = frame.clone();
        candidate["extra"] = json!(true);
        assert!(decode_host_frame(&candidate).is_err());
    }
    for (cwd, valid) in [
        (json!(null), false),
        (json!(""), false),
        (json!("😀".repeat(2048)), true),
        (json!("😀".repeat(2049)), false),
    ] {
        let mut candidate = call.clone();
        candidate["cwd"] = cwd;
        assert_eq!(decode_host_frame(&candidate).is_ok(), valid);
    }
    let mut candidate = service;
    candidate["method"] = json!("method.with.dots");
    assert!(decode_host_frame(&candidate).is_err());
}

#[test]
fn host_control_and_form_result_variants_decode_exactly() {
    for kind in ["cancel", "release", "admitted"] {
        let mut frame = json!({"kind":format!("client.capability.{kind}"), "invocationId":"i"});
        assert_eq!(
            serde_json::to_value(decode_host_frame(&frame).unwrap()).unwrap(),
            frame
        );
        frame["registrationId"] = json!("r");
        assert!(decode_host_frame(&frame).is_err());
    }
    let release = json!({"kind":"client.capability.registration_release", "registrationId":"r"});
    assert_eq!(
        serde_json::to_value(decode_host_frame(&release).unwrap()).unwrap(),
        release
    );
    for result in [
        json!({"action":"accept", "values":{"x":["a","b"]}}),
        json!({"action":"decline"}),
        json!({"action":"cancel"}),
    ] {
        let frame = json!({"kind":"client.capability.interaction_result", "invocationId":"i", "interactionId":"q", "result":result});
        assert_eq!(
            serde_json::to_value(decode_host_frame(&frame).unwrap()).unwrap(),
            frame
        );
    }
    assert!(decode_host_frame(&json!({"kind":"client.capability.interaction_result", "invocationId":"i", "interactionId":"q",
        "result":{"action":"cancel", "values":{}}})).is_err());
    let accepted = decode_client_frame(&json!({"kind":"client.capability.accepted", "invocationId":"i", "admissionEvidence":{"kind":"none"}})).unwrap();
    assert_eq!(accepted.invocation_id(), "i");
    assert!(decode_host_frame(&json!({"kind":"client.capability.accepted", "invocationId":"i", "admissionEvidence":{"kind":"none"}})).is_err());
}
