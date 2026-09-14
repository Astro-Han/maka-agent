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

use maka_protocol::capability::schema::validate;
use serde_json::json;

#[test]
fn accepts_source_schema_subset_and_local_pointer_semantics() {
    validate(&json!({
        "type": "object",
        "$defs": {"a/b~c": {"type": ["string", "null"], "examples": []}},
        "properties": {"value": {"$ref": "#/$defs/a~1b~0c"}},
        "patternProperties": {"(?<=a)b": true},
        "required": ["value", ""],
        "additionalProperties": false,
        "allOf": [true, {"items": [true, false], "additionalItems": false}],
        "minimum": -1, "minLength": 1.0,
        "default": {"arbitrary": 1}, "enum": [1, 1], "title": ""
    }))
    .unwrap();
    // Source allows an array as the final reference target, but not traversing it.
    validate(&json!({"type":"object", "$defs":{"x":{"enum":[1]}},
        "$ref":"#/$defs/x/enum"}))
    .unwrap();
    for pattern in [r"(?=a)(a)\1", r"(?<!a)b", r"\((?:a)[()]", "[]", "[[]"] {
        validate(&json!({"type":"object", "pattern":pattern})).unwrap();
    }
    let escaped = r"\(".repeat(256);
    validate(&json!({"type":"object", "pattern":escaped})).unwrap();
    let class = format!("[{}]", "(".repeat(200));
    validate(&json!({"type":"object", "pattern":class})).unwrap();
}

#[test]
fn rejects_schema_escape_paths_invalid_shapes_and_resource_exhaustion() {
    for schema in [
        json!({"type":["object"]}),
        json!({"type":"object", "$id":"x"}),
        json!({"type":"object", "properties":{"x":{"type":["string","string"]}}}),
        json!({"type":"object", "required":["x","x"]}),
        json!({"type":"object", "items":[]}),
        json!({"type":"object", "oneOf":[]}),
        json!({"type":"object", "multipleOf":0}),
        json!({"type":"object", "minLength":9007199254740992_u64}),
        json!({"type":"object", "pattern":"("}),
        json!({"type":"object", "patternProperties":{"[":true}}),
        json!({"type":"object", "$ref":"https://example.com/schema"}),
        json!({"type":"object", "$defs":{"x":true}, "$ref":"#/$defs/x~2"}),
        json!({"type":"object", "$defs":{"x":{"enum":[{}]}}, "$ref":"#/$defs/x/enum/0"}),
        json!({"type":"object", "$defs":{"x":{"const":null}}, "$ref":"#/$defs/x/const"}),
        json!({"type":"object", "default":{"":true}}),
        json!({"type":"object", "description":"x".repeat(32 * 1024)}),
        json!({"type":"object", "examples":vec![true; 8192]}),
    ] {
        assert!(validate(&schema).is_err(), "accepted {schema}");
    }
    let deep_pattern = format!("{}a{}", "(?:".repeat(8000), ")".repeat(8000));
    assert!(validate(&json!({"type":"object", "pattern":deep_pattern})).is_err());
    assert!(validate(&json!({"type":"object", "pattern":"a|".repeat(8000)})).is_err());
    let boundary = format!("{}a{}", "(?:".repeat(128), ")".repeat(128));
    validate(&json!({"type":"object", "pattern":boundary})).unwrap();
    let mut deep_value = json!(true);
    for _ in 0..33 {
        deep_value = json!({"items":deep_value});
    }
    assert!(validate(&json!({"type":"object", "items":deep_value})).is_err());
}
