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

use maka_protocol::access::*;
use maka_runtime::access::ManagedPrincipalKind;
use serde_json::{Value, json};

fn input() -> Value {
    json!({"principalKind":"remote_owner","principalId":"Owner_1.test:agent-2",
        "operationGrants":["future.operation"],"canPublishClientCapabilities":true,
        "canUseHostPaths":false})
}

fn output() -> Value {
    let mut value = input();
    value["credentialId"] = json!("credential: not a UUID");
    value["deliveryId"] = json!("delivery:😀");
    value
}

#[test]
fn issue_roundtrips_and_preserves_source_unknown_grants_and_id_semantics() {
    let request = input();
    let decoded = decode_issue_input(&request).unwrap();
    assert_eq!(decoded.principal_kind, ManagedPrincipalKind::RemoteOwner);
    assert_eq!(decoded.operation_grants, ["future.operation"]);
    assert_eq!(serde_json::to_value(decoded).unwrap(), request);
    let result = output();
    assert_eq!(
        serde_json::to_value(decode_issue_result(&result).unwrap()).unwrap(),
        result
    );
    for id in [json!("😀".repeat(64)), json!(" ")] {
        assert!(decode_revoke_input(&json!({"credentialId": id})).is_ok());
    }
    for id in [json!("😀".repeat(65)), json!(""), Value::Null, json!(1)] {
        assert!(decode_revoke_input(&json!({"credentialId": id})).is_err());
    }
}

#[test]
fn grant_and_principal_boundaries_match_wire_contract() {
    let mut value = input();
    value["operationGrants"] = json!((0..256).map(|n| format!("future.{n}")).collect::<Vec<_>>());
    value["principalId"] = json!("a".repeat(128));
    assert!(decode_issue_input(&value).is_ok());
    value["operationGrants"]
        .as_array_mut()
        .unwrap()
        .push(json!("extra"));
    assert!(decode_issue_input(&value).is_err());
    for grants in [json!([]), json!(["😀".repeat(64)])] {
        value["operationGrants"] = grants;
        assert!(decode_issue_input(&value).is_ok());
    }
    for grants in [
        json!(["😀".repeat(65)]),
        json!(["x", "x"]),
        json!([""]),
        json!([false]),
        Value::Null,
    ] {
        value["operationGrants"] = grants;
        assert!(decode_issue_input(&value).is_err());
    }
    value["operationGrants"] = json!([]);
    for principal in ["a".repeat(129), "é".into(), "has space".into(), "".into()] {
        value["principalId"] = json!(principal);
        assert!(decode_issue_input(&value).is_err());
    }
}

#[test]
fn optional_owner_is_provider_only_and_null_is_not_absence() {
    for bind in [None, Some(false), Some(true)] {
        let mut prepared = input();
        if let Some(bind) = bind {
            prepared["bindClientInstance"] = json!(bind);
        }
        assert_eq!(
            serde_json::to_value(decode_prepare_input(&prepared).unwrap()).unwrap(),
            prepared
        );
    }
    let mut prepared = input();
    prepared["bindClientInstance"] = Value::Null;
    assert!(decode_prepare_input(&prepared).is_err());
    prepared
        .as_object_mut()
        .unwrap()
        .remove("bindClientInstance");
    prepared["capabilityOwnerCredentialId"] = json!("owner");
    assert!(decode_prepare_input(&prepared).is_err());
    let mut request = input();
    request["capabilityOwnerCredentialId"] = Value::Null;
    assert!(decode_issue_input(&request).is_err());
    request["capabilityOwnerCredentialId"] = json!("owner:1");
    assert!(decode_issue_input(&request).is_err());
    request["principalKind"] = json!("capability_provider");
    assert_eq!(
        serde_json::to_value(decode_issue_input(&request).unwrap()).unwrap(),
        request
    );
    let mut result = output();
    result["capabilityOwner"] = Value::Null;
    assert!(decode_issue_result(&result).is_err());
    result["capabilityOwner"] = json!({"principalId":"owner:1","clientInstanceId":"😀".repeat(64)});
    assert!(decode_issue_result(&result).is_err());
    result["principalKind"] = json!("capability_provider");
    assert_eq!(
        serde_json::to_value(decode_issue_result(&result).unwrap()).unwrap(),
        result
    );
    for owner in [
        json!({"principalId":"owner","clientInstanceId":null}),
        json!({"principalId":"é","clientInstanceId":"id"}),
        json!({"principalId":"owner","clientInstanceId":"id","extra":true}),
    ] {
        result["capabilityOwner"] = owner;
        assert!(decode_issue_result(&result).is_err());
    }
}

#[test]
fn shapes_kinds_and_booleans_are_strict() {
    assert!(decode_finalize_input(&json!({})).is_ok());
    assert!(decode_finalize_input(&json!({"clientInstanceId":"self-reported"})).is_err());
    for value in [
        json!({}),
        json!({"reconnectRequired":null}),
        json!({"reconnectRequired":1}),
    ] {
        assert!(decode_finalize_result(&value).is_err());
    }
    for reconnect in [true, false] {
        let value = json!({"reconnectRequired": reconnect});
        assert_eq!(
            serde_json::to_value(decode_finalize_result(&value).unwrap()).unwrap(),
            value
        );
    }
    for field in [
        "principalKind",
        "principalId",
        "operationGrants",
        "canPublishClientCapabilities",
        "canUseHostPaths",
    ] {
        let mut request = input();
        request.as_object_mut().unwrap().remove(field);
        assert!(decode_issue_input(&request).is_err());
    }
    for (field, invalid) in [
        ("principalKind", json!("session_guest")),
        ("extra", json!(true)),
        ("canPublishClientCapabilities", json!(1)),
        ("canUseHostPaths", Value::Null),
    ] {
        let mut request = input();
        request[field] = invalid;
        assert!(decode_issue_input(&request).is_err());
    }
    let mut result = output();
    result["extra"] = json!(true);
    assert!(decode_issue_result(&result).is_err());
    assert!(decode_revoke_input(&json!({"credentialId":"id", "extra":true})).is_err());
    for revoked in [true, false] {
        let result = json!({"credentialId":"id", "revoked":revoked});
        assert_eq!(
            serde_json::to_value(decode_revoke_result(&result).unwrap()).unwrap(),
            result
        );
    }
    for result in [
        json!({"credentialId":"id", "revoked":1}),
        json!({"credentialId":"id"}),
        json!({"credentialId":"id", "revoked":false, "extra":true}),
    ] {
        assert!(decode_revoke_result(&result).is_err());
    }
}
