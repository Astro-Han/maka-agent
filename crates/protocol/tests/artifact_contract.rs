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

use maka_protocol::artifact::*;
use serde_json::{Value, json};
#[path = "support/artifact_source.rs"]
mod source;

#[test]
fn original_codec_agrees_on_upload_identity_bounds_and_chunk_encoding() {
    let mut cases = Vec::new();
    let mut add = |operation: &str, input: Value| {
        let decoded = match operation {
            "artifact.ingest" => {
                decode_ingest_input(&input).map(|input| serde_json::to_value(input).unwrap())
            }
            "artifact.query" => {
                decode_query_input(&input).map(|input| serde_json::to_value(input).unwrap())
            }
            "artifact.delete" => {
                decode_delete_input(&input).map(|input| serde_json::to_value(input).unwrap())
            }
            _ => unreachable!(),
        };
        let expected = match decoded {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(_) => json!({"ok":false}),
        };
        cases.push(json!({"operation":operation,"input":input,"expected":expected}));
    };
    let begin = json!({"kind":"begin","sessionId":"session","uploadId":"upload",
        "name":"file.txt","mimeType":"text/plain","totalBytes":0,"contentSha256":format!("sha256:{}", "0".repeat(64))});
    add("artifact.ingest", begin.clone());
    for (field, values) in [
        (
            "sessionId",
            vec![
                json!("a.b"),
                json!("a".repeat(128)),
                json!("a".repeat(129)),
                Value::Null,
            ],
        ),
        (
            "name",
            vec![
                json!(""),
                json!("😀".repeat(128)),
                json!("😀".repeat(129)),
                json!("line\n"),
                json!("\u{0085}"),
            ],
        ),
        (
            "mimeType",
            vec![
                json!(""),
                json!("x".repeat(256)),
                json!("x".repeat(257)),
                json!("text/plain\u{007f}"),
            ],
        ),
        (
            "totalBytes",
            vec![
                json!(1.0),
                json!(50 * 1024 * 1024),
                json!(50 * 1024 * 1024 + 1),
                json!(-1),
                json!(0.5),
                json!(9_007_199_254_740_992_u64),
            ],
        ),
        (
            "contentSha256",
            vec![
                json!("sha256:ABC"),
                json!(format!("sha256:{}", "A".repeat(64))),
                Value::Null,
            ],
        ),
        ("extra", vec![json!(true)]),
    ] {
        for value in values {
            let mut input = begin.clone();
            input[field] = value;
            add("artifact.ingest", input);
        }
    }
    for field in begin.as_object().unwrap().keys() {
        let mut input = begin.clone();
        input.as_object_mut().unwrap().remove(field);
        add("artifact.ingest", input);
    }
    for chunk in [
        "", "AA==", "AB==", "AAB=", "AA", "AA=", "AA==\n", "AAAA", "====", "AA-_",
    ] {
        add(
            "artifact.ingest",
            json!({"kind":"chunk","sessionId":"session","uploadId":"upload","offset":1.0,"chunkBase64":chunk}),
        );
    }
    for chunk in ["AAAA".repeat(16_384), "AAAA".repeat(16_385)] {
        add(
            "artifact.ingest",
            json!({"kind":"chunk","sessionId":"session","uploadId":"upload","offset":0,"chunkBase64":chunk}),
        );
    }
    for kind in ["commit", "abort"] {
        let input = json!({"kind":kind,"sessionId":"session","uploadId":"upload"});
        add("artifact.ingest", input.clone());
        let mut extra = input;
        extra["offset"] = json!(0);
        add("artifact.ingest", extra);
    }
    for kind in [
        "list_start",
        "get",
        "read_text",
        "read_binary",
        "read_chunk",
    ] {
        let mut input = json!({"kind":kind,"sessionId":"session"});
        if kind != "list_start" {
            input["artifactId"] = json!("artifact");
        }
        if kind == "read_chunk" {
            input["offset"] = json!(0.0);
        }
        add("artifact.query", input.clone());
        input["sessionId"] = json!("../other");
        add("artifact.query", input);
    }
    for cursor in ["0", "01", "-1", "", "任意"] {
        add(
            "artifact.query",
            json!({"kind":"list_continue","sessionId":"session",
            "revision":format!("sha256:{}", "0".repeat(64)),"cursor":cursor}),
        );
    }
    add(
        "artifact.delete",
        json!({"sessionId":"session","artifactId":"artifact"}),
    );
    add(
        "artifact.delete",
        json!({"sessionId":"session","artifactId":"artifact","extra":true}),
    );
    source::compare(&cases);
}
