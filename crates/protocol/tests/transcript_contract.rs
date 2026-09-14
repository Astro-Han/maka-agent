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
use maka_protocol::transcript::*;
use serde_json::{Value, json};

fn page() -> Value {
    json!({"kind":"page","sessionId":"session_1","source":"durable","direction":"older",
        "throughSequence":8,"rawBytes":1,"fragments":[{"kind":"durable","sequence":8,
        "byteOffset":1,"totalBytes":3,"payloadDigest":null,"data":"uA=="}],
        "rangeBoundarySequence":8,"protectedTurnSequence":8,"nextCursor":null})
}
fn input() -> Value {
    json!({"subscriptionId":"sub","source":"durable","direction":"older",
        "throughSequence":8,"cursor":null,"anchorSequence":null,"maxBytes":1})
}

#[test]
fn exact_nullable_fields_numeric_spellings_and_wire_constants() {
    let mut value = page();
    value["throughSequence"] = json!(8.0);
    let decoded = decode_session_transcript_page(&value).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), page());
    for field in [
        "nextCursor",
        "throughSequence",
        "protectedTurnSequence",
        "rangeBoundarySequence",
    ] {
        let mut bad = page();
        bad.as_object_mut().unwrap().remove(field);
        assert!(decode_session_transcript_page(&bad).is_err(), "{field}");
    }
    for bad in [json!(true), json!(null), json!("other")] {
        let mut value = page();
        value["kind"] = bad;
        assert!(decode_session_transcript_page(&value).is_err());
    }
    for (field, bad) in [
        ("rawBytes", json!(true)),
        ("throughSequence", json!(9007199254740992u64)),
        ("partial", json!(true)),
    ] {
        let mut value = page();
        value[field] = bad;
        assert!(decode_session_transcript_page(&value).is_err(), "{field}");
    }
    let mut value = input();
    value["maxBytes"] = json!(1.0);
    assert_eq!(
        decode_session_transcript_page_input(&value)
            .unwrap()
            .max_bytes,
        1
    );
    value["cursor"] = json!("cursor");
    value["anchorSequence"] = json!(0);
    assert!(decode_session_transcript_page_input(&value).is_err());
}

#[test]
fn fragments_are_bytes_and_require_canonical_base64_bounds_and_order() {
    // A fragment can contain only the middle byte of 中; UTF-8 is decoded after assembly.
    assert_eq!("中".as_bytes()[1], STANDARD.decode("uA==").unwrap()[0]);
    assert!(decode_session_transcript_page(&page()).is_ok());
    for data in ["uB==", "uA", "uA==\n", "", "_A=="] {
        let mut value = page();
        value["fragments"][0]["data"] = json!(data);
        assert!(decode_session_transcript_page(&value).is_err(), "{data}");
    }
    for (field, bad) in [
        ("byteOffset", json!(3)),
        ("totalBytes", json!(0)),
        ("sequence", json!(9)),
        ("payloadDigest", json!("sha256:ABC")),
        ("unknown", json!(0)),
    ] {
        let mut value = page();
        value["fragments"][0][field] = bad;
        assert!(decode_session_transcript_page(&value).is_err(), "{field}");
    }
    let mut value = page();
    value["fragments"][0]["payloadDigest"] = json!(format!("sha256:{}", "a".repeat(64)));
    assert!(decode_session_transcript_page(&value).is_ok());
    let second = value["fragments"][0].clone();
    value["fragments"].as_array_mut().unwrap().push(second);
    value["rawBytes"] = json!(2);
    assert!(decode_session_transcript_page(&value).is_err());
    value["fragments"][1]["sequence"] = json!(7);
    assert!(decode_session_transcript_page(&value).is_ok());
    value["direction"] = json!("newer");
    assert!(decode_session_transcript_page(&value).is_err());
    let mut large = page();
    large["fragments"][0]["data"] =
        json!(STANDARD.encode(vec![0; SESSION_TRANSCRIPT_PAGE_MAX_BYTES as usize + 1]));
    assert!(decode_session_transcript_page(&large).is_err());
}

#[test]
fn page_and_bootstrap_match_identity_watermark_and_combined_budget() {
    let request = decode_session_transcript_page_input(&input()).unwrap();
    let result = decode_session_transcript_page(&page()).unwrap();
    assert!(assert_page_for_input(&request, &result).is_ok());
    let mut changed = result.clone();
    changed.through_sequence = Some(9);
    assert!(assert_page_for_input(&request, &changed).is_err());
    let mut overlay = page();
    overlay["source"] = json!("overlay");
    overlay["fragments"] =
        json!([{"kind":"overlay","messageIndex":0,"byteOffset":0,"totalBytes":1,"data":"eA=="}]);
    overlay["rangeBoundarySequence"] = Value::Null;
    overlay["protectedTurnSequence"] = Value::Null;
    let value =
        json!({"throughSequence":8,"overlayMessageCount":1,"durable":page(),"overlay":overlay});
    let bootstrap = decode_session_transcript_bootstrap(&value).unwrap();
    assert!(validate_bootstrap_for_input(&bootstrap, "session_1", 2).is_ok());
    assert!(validate_bootstrap_for_input(&bootstrap, "wrong", 2).is_err());
    let mut changed = bootstrap.clone();
    changed.overlay.raw_bytes = 2;
    assert!(validate_bootstrap_for_input(&changed, "session_1", 2).is_err());
    let mut changed = value.clone();
    changed["overlay"]["throughSequence"] = json!(7);
    assert!(decode_session_transcript_bootstrap(&changed).is_err());
    let mut changed = value.clone();
    changed["overlayMessageCount"] = json!(4097);
    assert!(decode_session_transcript_bootstrap(&changed).is_err());
    let mut changed = value;
    changed["overlay"]["rangeBoundarySequence"] = json!(0);
    assert!(decode_session_transcript_bootstrap(&changed).is_err());
}

#[test]
fn empty_pages_are_explicit_and_release_shape_is_exact() {
    let release =
        decode_session_transcript_overlay_release_input(&json!({"subscriptionId":"sub"})).unwrap();
    let other =
        decode_session_transcript_overlay_release_result(&json!({"subscriptionId":"other"}))
            .unwrap();
    assert!(assert_overlay_release_for_input(&release, &release).is_ok());
    assert!(assert_overlay_release_for_input(&release, &other).is_err());
    assert!(decode_session_transcript_page(&json!({})).is_err());
    let mut empty = page();
    empty["throughSequence"] = Value::Null;
    empty["fragments"] = json!([]);
    empty["rawBytes"] = json!(0);
    empty["rangeBoundarySequence"] = Value::Null;
    empty["protectedTurnSequence"] = Value::Null;
    let decoded = decode_session_transcript_page(&empty).unwrap();
    assert!(decoded.fragments.is_empty());
    assert_eq!(serde_json::to_value(decoded).unwrap(), empty);
    empty["nextCursor"] = json!("unreachable");
    assert!(decode_session_transcript_page(&empty).is_err());
    assert!(
        decode_session_transcript_overlay_release_input(&json!({"subscriptionId":"sub"})).is_ok()
    );
    assert!(
        decode_session_transcript_overlay_release_result(
            &json!({"subscriptionId":"sub","released":true})
        )
        .is_err()
    );
}
