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

use maka_protocol::{codec, handshake::*, *};
use serde_json::{Value, json};

// Source oracle: protocol/index.ts decodeClientFrame, client/connection.ts
// exchangeRuntimeHostHandshake; supported domains aligned to d2e6c1f27.
fn hello() -> Value {
    json!({"kind":"hello", "clientInstanceId":"desktop-instance",
        "protocolMin":0, "protocolMax":0,
        "compatibilityEpoch":COMPATIBILITY_EPOCH, "compositionId":"maka.interactive"})
}

#[test]
fn hello_is_closed_and_negotiates_before_admission() {
    let mut wire = hello();
    let parsed = decode_hello(&wire).unwrap();
    let host = ProtocolRange { min: 0, max: 0 };
    assert_eq!(
        parsed
            .negotiate(host, COMPATIBILITY_EPOCH, COMPOSITION_ID)
            .unwrap(),
        Some(0)
    );
    assert_eq!(parsed.negotiate(host, 139, COMPOSITION_ID).unwrap(), None);
    assert_eq!(
        parsed
            .negotiate(host, COMPATIBILITY_EPOCH, "other.host")
            .unwrap(),
        None
    );
    let normalized = decode_message(&encode_message(&parsed).unwrap()).unwrap();
    assert_eq!(normalized["kind"], "hello");

    for (field, value, valid) in [
        ("clientInstanceId", json!("😀".repeat(64)), true),
        ("clientInstanceId", json!("😀".repeat(65)), false),
        ("protocolMax", json!(9_007_199_254_740_991u64), true),
        ("protocolMax", json!(9_007_199_254_740_992u64), false),
        ("protocolMin", json!(-1), false),
        ("protocolMin", json!(0.0), true),
        ("protocolMin", json!(0.5), false),
        ("compatibilityEpoch", json!(1_000_001), false),
        ("compositionId", json!("maka..interactive"), false),
        ("compositionId", Value::Null, false),
        ("activitySnapshotVersion", json!(2), false),
        ("surface", json!("desktop"), false),
        ("futureField", json!(true), false),
        ("takeover", json!({"expectedHostEpoch":"old"}), false),
    ] {
        let mut frame = hello();
        frame[field] = value;
        assert_eq!(decode_hello(&frame).is_ok(), valid, "{field}: {frame}");
    }
    for field in ["compatibilityEpoch", "compositionId"] {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(decode_hello(&missing).is_err(), "{field}");
    }
    wire["generation"] = json!("next");
    wire["takeover"] = json!({"expectedHostEpoch":"old", "unknown":true});
    assert!(decode_hello(&wire).is_err());
    wire["takeover"] = json!({"expectedHostEpoch":"old"});
    assert!(decode_hello(&wire).unwrap().takeover.is_some());
}

#[test]
fn host_handshake_roundtrips_and_rejects_invalid_nested_evidence() {
    let accepted = json!({"kind":"accepted", "rootId":"a".repeat(64), "hostEpoch":"epoch",
        "connectionId":"connection", "selectedProtocol":0, "compatibilityEpoch":COMPATIBILITY_EPOCH,
        "compositionId":"maka.interactive", "compositionRevision":"3", "state":"ready"});
    let incompatible = json!({"kind":"incompatible", "hostEpoch":"epoch", "protocolMin":0,
        "protocolMax":0, "state":"ready", "replacement":"wait_for_idle_exit",
        "compatibilityEpoch":COMPATIBILITY_EPOCH,"compositionId":COMPOSITION_ID,"compositionRevision":"3",
        "activity":{"connections":0, "activeOperations":0, "processUptimeSeconds":1,
        "residencies":[], "drainResidencies":0, "cooperativeHandoff":false}});
    let draining = json!({"kind":"draining", "hostEpoch":"epoch", "compositionId":COMPOSITION_ID,"compositionRevision":"3"});
    for frame in [&accepted, &incompatible, &draining] {
        let decoded = decode_host_handshake(frame).unwrap();
        let encoded = decode_message(&encode_message(&decoded).unwrap()).unwrap();
        assert_eq!(decode_host_handshake(&encoded).unwrap(), decoded);
    }
    for (field, value) in [
        ("state", json!("draining")),
        ("rootId", json!("A".repeat(64))),
        ("cooperativeHandoff", json!(false)),
        ("compositionRevision", json!("3\u{7f}")),
    ] {
        let mut frame = accepted.clone();
        frame[field] = value;
        assert!(decode_host_handshake(&frame).is_err(), "{field}");
    }
    let mut frame = incompatible;
    frame["activity"]["unknown"] = json!(true);
    assert!(decode_host_handshake(&frame).is_err());
}

// Deliberately a test-only partial registry; the production crate supplies no
// pretend implementations. Empty host.status input and error codes match host-status.ts.
struct StatusRegistry;
impl OperationRegistry for StatusRegistry {
    fn error_codes(&self, operation: Operation) -> Option<&[OperationErrorCode]> {
        (operation == Operation::HostStatus).then_some(&[
            OperationErrorCode::HostDraining,
            OperationErrorCode::InternalFailure,
        ])
    }
    fn decode_input(&self, _: Operation, value: &Value) -> Result<Value> {
        codec::exact(codec::record(value, "host.status input")?, &[])?;
        Ok(value.clone())
    }
    fn decode_output(&self, _: Operation, _: &Value) -> Result<Value> {
        Err(ProtocolError::invalid(
            "Output decoder not supplied in this fixture",
        ))
    }
}

#[test]
fn strict_operation_envelopes_and_message_errors() {
    let request = json!({"requestId":"1", "operation":"host.status", "input":{}});
    assert!(decode_request(&request, &StatusRegistry).is_ok());
    for (field, value) in [
        ("kind", json!("request")),
        ("input", json!({"extra":1})),
        ("operation", json!("unknown")),
        ("requestId", json!("")),
    ] {
        let mut frame = request.clone();
        frame[field] = value;
        assert!(decode_request(&frame, &StatusRegistry).is_err(), "{field}");
    }
    for (code, valid) in [
        ("host_draining", true),
        ("unauthorized", true),
        ("operation_unavailable", false),
        ("invented_error", false),
    ] {
        let mut frame = json!({"requestId":"1", "operation":"host.status", "ok":false,
            "error":{"code":code,"message":"Unavailable"}});
        assert_eq!(
            decode_response(&frame, &StatusRegistry).is_ok(),
            valid,
            "{code}"
        );
        if valid {
            let response = decode_response(&frame, &StatusRegistry).unwrap();
            assert_eq!(serde_json::to_value(response).unwrap(), frame);
        }
        frame["error"]["extra"] = json!(true);
        assert!(decode_response(&frame, &StatusRegistry).is_err());
    }
    assert_eq!(
        decode_message(&[0xff]).unwrap_err().code,
        ErrorCode::InvalidUtf8
    );
    assert_eq!(
        decode_message(b"{").unwrap_err().code,
        ErrorCode::InvalidJson
    );
    assert_eq!(
        decode_message(&vec![b' '; MAX_MESSAGE_BYTES + 1])
            .unwrap_err()
            .code,
        ErrorCode::FrameTooLarge
    );
    assert!(encode_message(&"x".repeat(MAX_MESSAGE_BYTES - 2)).is_ok());
    assert_eq!(
        encode_message(&"x".repeat(MAX_MESSAGE_BYTES - 1))
            .unwrap_err()
            .code,
        ErrorCode::FrameTooLarge
    );
}
