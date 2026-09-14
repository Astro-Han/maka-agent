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

use maka_protocol::capability::{decode_form_input, decode_form_result};
use serde_json::{Value, json};

fn request(field: Value) -> Value {
    json!({"message":"Complete","requester":{"name":"Tool","source":""},"fields":[field]})
}
fn string_field() -> Value {
    json!({"kind":"string","name":"answer","label":"Answer","required":true,"maxLength":32})
}
#[test]
fn admits_closed_variants_and_projects_display_without_redacting_identity() {
    for field in [
        string_field(),
        json!({"kind":"number","name":"x","label":"X","required":true,"minimum":0.2}),
        json!({"kind":"integer","name":"x","label":"X","required":true,"minimum":0.2,"maximum":1.1}),
        json!({"kind":"boolean","name":"x","label":"X","required":true,"default":false}),
        json!({"kind":"single_select","name":"x","label":"X","required":true,"options":[{"value":"","label":"Empty"}],"default":""}),
        json!({"kind":"multi_select","name":"x","label":"X","required":true,"options":[{"value":"a","label":"A"}],"minItems":1}),
    ] {
        assert!(decode_form_input(&request(field)).is_ok());
    }
    let mut field = string_field();
    field["label"] = json!("Password\n");
    field["default"] = json!("unsafe\n");
    field["name"] = json!("identity\n");
    let decoded = serde_json::to_value(decode_form_input(&request(field)).unwrap()).unwrap();
    assert_eq!(decoded["fields"][0]["label"], json!("Password\\u{A}"));
    assert_eq!(decoded["fields"][0]["name"], json!("identity\n"));
    assert!(decoded["fields"][0].get("default").is_none());
    let mut unredacted = request(string_field());
    unredacted["message"] = json!("password=secret");
    assert_eq!(
        decode_form_input(&unredacted).unwrap().message,
        "password=secret"
    );
}
#[test]
fn rejects_unsatisfiable_required_fields_and_reserves_optional_answer_envelopes() {
    let mut prototype = string_field();
    prototype["name"] = json!("__proto__");
    assert!(decode_form_input(&request(prototype)).is_err());
    let mut field = string_field();
    field["format"] = json!("uri");
    field["maxLength"] = json!(12);
    assert!(decode_form_input(&request(field)).is_err());
    let integer = json!({"kind":"integer","name":"x","label":"X","required":true,"minimum":0.1,"maximum":0.9});
    assert!(decode_form_input(&request(integer.clone())).is_err());
    let mut optional = integer;
    optional["required"] = json!(false);
    assert!(decode_form_input(&request(optional)).is_ok());
    let mut field = string_field();
    field["required"] = json!(false);
    field.as_object_mut().unwrap().remove("maxLength");
    assert!(decode_form_input(&request(field)).is_err());
    let mut duplicate = request(string_field());
    duplicate["fields"] = json!([string_field(), string_field()]);
    assert!(decode_form_input(&duplicate).is_err());
}
#[test]
fn validates_unicode_byte_limits_formats_and_exact_shapes() {
    for (format, default, accepted) in [
        ("email", "a@b.co", true),
        ("email", "a b@c.co", false),
        ("date", "2000-02-29", true),
        ("date", "1900-02-29", false),
        ("date-time", "0000-01-01T00:00:00+23:59", true),
        ("date-time", "2000-01-01T24:00:00Z", false),
        ("uri", "mailto:x@y.co", true),
        ("uri", "relative/path", false),
    ] {
        let mut field = string_field();
        field["format"] = json!(format);
        field["default"] = json!(default);
        assert_eq!(
            decode_form_input(&request(field)).is_ok(),
            accepted,
            "{format}: {default}"
        );
    }
    let mut field = string_field();
    field["label"] = json!("界".repeat(86));
    assert!(decode_form_input(&request(field)).is_err());
    let mut field = string_field();
    field["default"] = Value::Null;
    assert!(decode_form_input(&request(field)).is_err());
    let mut field = string_field();
    field["extra"] = json!(true);
    assert!(decode_form_input(&request(field)).is_err());
}
#[test]
fn accepts_only_bounded_form_results_including_canonical_outcome_overhead() {
    for result in [
        json!({"action":"cancel"}),
        json!({"action":"decline","kind":"form"}),
        json!({"action":"accept","values":{"s":"","n":1.5,"b":false,"a":["a","b"]}}),
    ] {
        assert!(decode_form_result(&result).is_ok());
    }
    for result in [
        json!({"action":"cancel","values":{}}),
        json!({"action":"accept","values":{"a":["x","x"]}}),
        json!({"action":"accept","values":{"a":null}}),
        json!({"action":"decline","kind":"question"}),
    ] {
        assert!(decode_form_result(&result).is_err());
    }
    let values = json!({"a":"a".repeat(2048),"b":"b".repeat(2048),"c":"c".repeat(2048),"d":"d".repeat(1980)});
    assert!(decode_form_result(&json!({"action":"accept","values":values})).is_err());
}
