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

use maka_protocol::connection_effects::{
    ConnectionTestProjection, ConnectionTestRunInput, decode_connection_model_fetch_input as input,
    decode_connection_model_fetch_result as result, decode_connection_test_run_input as test_input,
    decode_connection_test_run_result as test_result,
};
use serde_json::{Value, json};

fn committed() -> Value {
    json!({"kind":"committed","catalogRevision":0,"connection":{
        "connectionId":"11111111-1111-4111-8111-111111111111","revision":1
    },"modelCount":1,"source":"fetched","fetchedAt":0})
}

#[test]
fn input_preserves_protocol_entity_ids_and_rejects_extra_fields() {
    for id in ["a", "fixture_id-01", &"a".repeat(128)] {
        let wire = json!({"connectionId":id});
        assert_eq!(serde_json::to_value(input(&wire).unwrap()).unwrap(), wire);
    }
    for wire in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"connectionId":null}),
        json!({"connectionId":""}),
        json!({"connectionId":"a.b"}),
        json!({"connectionId":"é"}),
        json!({"connectionId":"a".repeat(129)}),
        json!({"connectionId":"a","extra":null}),
    ] {
        assert!(input(&wire).is_err(), "{wire}");
    }
}

#[test]
fn every_result_variant_round_trips_and_changed_order_is_preserved() {
    let mut wires = vec![committed()];
    let mut fallback = committed();
    fallback["source"] = json!("fallback");
    wires.push(fallback);
    for reason in [
        "connection_not_found",
        "connection_disabled",
        "provider_action_unavailable",
        "credential_not_configured",
    ] {
        wires.push(json!({"kind":"rejected","reason":reason}));
    }
    for class in [
        "auth",
        "timeout",
        "provider_unavailable",
        "network",
        "invalid_response",
        "unknown",
    ] {
        wires.push(json!({"kind":"failed","errorClass":class}));
    }
    for changed in [
        json!(["connection"]),
        json!(["credential"]),
        json!(["network_proxy"]),
        json!(["network_proxy", "credential", "connection"]),
    ] {
        wires.push(json!({"kind":"superseded","changed":changed}));
    }
    for wire in wires {
        assert_eq!(serde_json::to_value(result(&wire).unwrap()).unwrap(), wire);
        for key in wire.as_object().unwrap().keys() {
            let mut missing = wire.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(result(&missing).is_err(), "{missing}");
        }
        let mut extra = wire;
        extra["extra"] = Value::Null;
        assert!(result(&extra).is_err(), "{extra}");
    }
    for wire in [
        Value::Null,
        json!([]),
        json!({"kind":"unknown"}),
        json!({"kind":"rejected","reason":"unknown"}),
        json!({"kind":"failed","errorClass":"rejected"}),
        json!({"kind":"superseded","changed":[]}),
        json!({"kind":"superseded","changed":["connection","connection"]}),
        json!({"kind":"superseded","changed":["connection","credential","network_proxy","connection"]}),
        json!({"kind":"superseded","changed":["unknown"]}),
        json!({"kind":"superseded","changed":null}),
        json!({"kind":"superseded","changed":[1]}),
    ] {
        assert!(result(&wire).is_err(), "{wire}");
    }
}

#[test]
fn committed_numeric_limits_match_javascript_safe_integer_semantics() {
    let mut wire = committed();
    for field in ["catalogRevision", "modelCount", "fetchedAt"] {
        for invalid in [
            json!(-1),
            json!(1.5),
            json!(9_007_199_254_740_992_u64),
            Value::Null,
            json!("1"),
        ] {
            let mut bad = wire.clone();
            bad[field] = invalid;
            assert!(result(&bad).is_err(), "{bad}");
        }
        wire[field] = json!(1.0);
    }
    wire["connection"]["revision"] = json!(1.0);
    let decoded = serde_json::to_value(result(&wire).unwrap()).unwrap();
    assert_eq!(decoded["connection"]["revision"], json!(1));
    for field in ["catalogRevision", "fetchedAt"] {
        wire[field] = json!(9_007_199_254_740_991_u64);
    }
    wire["modelCount"] = json!(2048);
    wire["connection"]["revision"] = json!(9_007_199_254_740_991_u64);
    assert!(result(&wire).is_ok());
    for count in [0, 2049] {
        wire["modelCount"] = json!(count);
        assert!(result(&wire).is_err());
    }
    for revision in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(9_007_199_254_740_992_u64),
    ] {
        let mut bad = committed();
        bad["connection"]["revision"] = revision;
        assert!(result(&bad).is_err(), "{bad}");
    }
    for basis in [
        json!({"connectionId":"not-a-uuid","revision":1}),
        json!({"connectionId":"11111111-1111-4111-8111-111111111111"}),
        json!({"connectionId":"11111111-1111-4111-8111-111111111111","revision":1,"extra":true}),
    ] {
        let mut bad = committed();
        bad["connection"] = basis;
        assert!(result(&bad).is_err(), "{bad}");
    }
    let mut bad = committed();
    bad["source"] = json!("cached");
    assert!(result(&bad).is_err());
}

fn test_committed(failed: bool) -> Value {
    let test = if failed {
        json!({"kind":"failed","checkedAt":"now","modelId":null,
            "latencyMs":null,"statusCode":null,"errorClass":"invalid_response"})
    } else {
        json!({"kind":"verified","checkedAt":"now","modelId":"provider/model","latencyMs":0})
    };
    json!({"kind":"committed","catalogRevision":0,"connection":{
        "connectionId":"11111111-1111-4111-8111-111111111111","revision":1
    },"test":test})
}

#[test]
fn test_inputs_require_nullable_model_and_preserve_model_strings() {
    for model in [
        Value::Null,
        json!("provider/model"),
        json!("🦀".repeat(256)),
    ] {
        let wire = json!({"connectionId":"fixture_id-01","modelId":model});
        assert_eq!(
            serde_json::to_value(test_input(&wire).unwrap()).unwrap(),
            wire
        );
    }
    for wire in [
        json!({"connectionId":"fixture"}),
        json!({"connectionId":"fixture","modelId":""}),
        json!({"connectionId":"fixture","modelId":"🦀".repeat(257)}),
        json!({"connectionId":"fixture","modelId":42}),
        json!({"connectionId":"a.b","modelId":null}),
        json!({"connectionId":"fixture","modelId":null,"extra":true}),
    ] {
        assert!(test_input(&wire).is_err(), "{wire}");
    }
    assert!(
        serde_json::from_value::<ConnectionTestRunInput>(json!({"connectionId":"fixture"}))
            .is_err()
    );
}

#[test]
fn test_results_preserve_variants_and_require_every_nested_field() {
    for wire in [
        test_committed(false),
        test_committed(true),
        json!({"kind":"rejected","reason":"credential_not_configured"}),
        json!({"kind":"superseded","changed":["network_proxy","connection"]}),
    ] {
        assert_eq!(
            serde_json::to_value(test_result(&wire).unwrap()).unwrap(),
            wire
        );
        for key in wire.as_object().unwrap().keys() {
            let mut bad = wire.clone();
            bad.as_object_mut().unwrap().remove(key);
            assert!(test_result(&bad).is_err(), "{bad}");
        }
        if wire["kind"] != "committed" {
            continue;
        }
        for field in ["test", "connection"] {
            for key in wire[field].as_object().unwrap().keys() {
                let mut bad = wire.clone();
                bad[field].as_object_mut().unwrap().remove(key);
                assert!(test_result(&bad).is_err(), "{bad}");
                if field == "test" {
                    assert!(
                        serde_json::from_value::<ConnectionTestProjection>(bad[field].clone())
                            .is_err()
                    );
                }
            }
            let mut bad = wire.clone();
            bad[field]["extra"] = Value::Null;
            assert!(test_result(&bad).is_err(), "{bad}");
        }
    }
    for wire in [
        json!({"kind":"failed","errorClass":"auth"}),
        json!({"kind":"superseded","changed":["connection","connection"]}),
        json!({"kind":"rejected","reason":"unknown"}),
    ] {
        assert!(test_result(&wire).is_err(), "{wire}");
    }
    for (field, invalid) in [
        ("kind", json!("unknown")),
        ("checkedAt", json!("")),
        ("checkedAt", json!("a".repeat(129))),
        ("modelId", json!("")),
        ("latencyMs", json!(-1)),
        ("latencyMs", json!(1.5)),
        ("latencyMs", json!(9_007_199_254_740_992_u64)),
        ("statusCode", json!(99)),
        ("statusCode", json!(600)),
        ("statusCode", json!(401.5)),
        ("errorClass", json!("needs_reauth")),
    ] {
        let mut bad = test_committed(true);
        bad["test"][field] = invalid;
        assert!(test_result(&bad).is_err(), "{bad}");
    }
}

#[test]
fn test_nested_counts_accept_integral_floats_and_safe_integer_boundary() {
    for failed in [false, true] {
        let mut wire = test_committed(failed);
        wire["catalogRevision"] = json!(1.0);
        wire["connection"]["revision"] = json!(1.0);
        wire["test"]["latencyMs"] = json!(1.0);
        if failed {
            wire["test"]["statusCode"] = json!(401.0);
        }
        let decoded = serde_json::to_value(test_result(&wire).unwrap()).unwrap();
        assert_eq!(decoded["catalogRevision"], json!(1));
        assert_eq!(decoded["connection"]["revision"], json!(1));
        assert_eq!(decoded["test"]["latencyMs"], json!(1));
        if failed {
            assert_eq!(decoded["test"]["statusCode"], json!(401));
        }
        wire["test"]["latencyMs"] = json!(9_007_199_254_740_991_u64);
        assert!(test_result(&wire).is_ok());
    }
}
