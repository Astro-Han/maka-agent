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

use maka_protocol::capability::decode_result;
use serde_json::{Value, json};

#[test]
fn rich_result_roundtrips_without_losing_absence_or_explicit_null() {
    let value = json!({
        "content": [
            {"type": "text", "text": ""},
            {"type": "image", "data": "AQI=", "mimeType": "image/svg+xml"},
            {"type": "audio", "data": "AAAA", "mimeType": "custom"},
            {"type": "resource", "uri": "file:///a"},
            {"type": "resource", "uri": "urn:a", "mimeType": "text/plain", "text": "", "blob": "AQ=="},
            {"type": "resource_link", "uri": "urn:b", "name": "b", "description": "linked", "mimeType": "text/plain"},
            {"type": "unknown", "value": {"__proto__": {"safe": true}, "nested": [null, 4, "😀"]}}
        ],
        "structuredContent": null
    });
    let decoded = decode_result(&value).unwrap();
    assert_eq!(decoded.structured_content, Some(Value::Null));
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
    let absent = json!({"content": []});
    assert_eq!(
        serde_json::to_value(decode_result(&absent).unwrap()).unwrap(),
        absent
    );

    // String limits count UTF-16 units, and JSON depth starts at zero.
    let link =
        json!({"content": [{"type": "resource_link", "uri": "x", "name": "😀".repeat(256)}]});
    assert!(decode_result(&link).is_ok());
    let nested = (0..32).fold(Value::Null, |value, _| json!([value]));
    assert!(decode_result(&json!({"content": [], "structuredContent": nested})).is_ok());
}

#[test]
fn rejects_noncanonical_media_and_malformed_result_structure() {
    for data in ["", "AQ", "AQ=", "AR==", "AAB=", "AQ==\n", "_w==", "===="] {
        let value = json!({"content": [{"type": "image", "data": data, "mimeType": "image/png"}]});
        assert!(decode_result(&value).is_err(), "accepted base64 {data:?}");
    }
    for mime in [
        "Image/png",
        "image/",
        "image/.png",
        "image/png;foo",
        "image/png\n",
    ] {
        let value = json!({"content": [{"type": "image", "data": "AQ==", "mimeType": mime}]});
        assert!(decode_result(&value).is_err(), "accepted MIME {mime:?}");
    }
    for block in [
        json!({"type": "text", "text": "ok", "extra": false}),
        json!({"type": "resource", "uri": "x", "text": null}),
        json!({"type": "resource_link", "uri": "x", "name": null}),
        json!({"type": "resource_link", "uri": "x", "name": "😀".repeat(257)}),
        json!({"type": "unknown", "value": {"": 1}}),
        json!({"type": "future", "value": null}),
    ] {
        assert!(decode_result(&json!({"content": [block]})).is_err());
    }
    assert!(decode_result(&json!({"content": [], "extra": 0})).is_err());
    assert!(
        decode_result(&json!({"content": vec![json!({"type": "text", "text": ""}); 257]})).is_err()
    );
    let nested = (0..33).fold(Value::Null, |value, _| json!([value]));
    for value in [nested, json!(vec![0; 8192]), json!({"x".repeat(257): null})] {
        assert!(decode_result(&json!({"content": [], "structuredContent": value})).is_err());
        assert!(decode_result(&json!({"content": [{"type": "unknown", "value": value}]})).is_err());
    }
}
