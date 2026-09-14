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
fn original_codec_agrees_on_required_nulls_receipts_and_bounded_previews() {
    let mut cases = Vec::new();
    let mut add = |operation: &str, input: Value| {
        let decoded = match operation {
            "artifact.ingest" => {
                decode_ingest_result(&input).map(|v| serde_json::to_value(v).unwrap())
            }
            "artifact.query" => {
                decode_query_result(&input).map(|v| serde_json::to_value(v).unwrap())
            }
            "artifact.delete" => {
                decode_delete_result(&input).map(|v| serde_json::to_value(v).unwrap())
            }
            _ => unreachable!(),
        };
        let expected = match decoded {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(_) => json!({"ok":false}),
        };
        cases.push(
            json!({"operation":operation,"direction":"output","input":input,"expected":expected}),
        );
    };
    let revision = format!("sha256:{}", "0".repeat(64));
    let artifact = json!({"id":"artifact","sessionId":"session","turnId":"turn.with.😀",
        "createdAt":1.0,"name":"file","kind":"file","sizeBytes":0.0,"source":"user_upload"});
    let item =
        json!({"kind":"artifact","sessionId":"session","revision":revision,"artifact":artifact});
    add("artifact.query", item.clone());
    for (field, values) in [
        (
            "turnId",
            vec![
                json!("😀".repeat(256)),
                json!("😀".repeat(257)),
                json!("\u{7f}"),
                json!(""),
            ],
        ),
        (
            "name",
            vec![
                json!("😀".repeat(128)),
                json!("😀".repeat(129)),
                json!("\n"),
            ],
        ),
        (
            "mimeType",
            vec![json!("x".repeat(512)), json!("x".repeat(513)), Value::Null],
        ),
        (
            "summary",
            vec![
                json!("x".repeat(8192)),
                json!("x".repeat(8193)),
                Value::Null,
            ],
        ),
        (
            "sizeBytes",
            vec![
                json!(9_007_199_254_740_991_u64),
                json!(9_007_199_254_740_992_u64),
                json!(0.5),
            ],
        ),
        ("source", vec![json!("session_effect"), json!("unknown")]),
        ("extra", vec![json!(true)]),
    ] {
        for value in values {
            let mut output = item.clone();
            output["artifact"][field] = value;
            add("artifact.query", output);
        }
    }
    let page = json!({"kind":"page","sessionId":"session","revision":revision,
        "artifacts":[artifact],"nextCursor":null});
    let chunk = json!({"kind":"chunk","sessionId":"session","artifactId":"artifact",
        "offset":0.0,"totalBytes":1.0,"chunkBase64":"AB==","nextOffset":null});
    for (output, nullable) in [
        (item.clone(), "artifact"),
        (page.clone(), "nextCursor"),
        (chunk.clone(), "nextOffset"),
    ] {
        add("artifact.query", output.clone());
        let mut changed = output.clone();
        changed[nullable] = Value::Null;
        add("artifact.query", changed);
        let mut changed = output;
        changed.as_object_mut().unwrap().remove(nullable);
        add("artifact.query", changed);
    }
    for (total, data, next) in [
        (2, "AB==", json!(1)),
        (2, "AB==", Value::Null),
        (1, "AB==", json!(1)),
        (2, "", json!(0)),
        (0, "", Value::Null),
        (0, "AA==", Value::Null),
        (1, "AA", Value::Null),
        (1, "AA==\n", Value::Null),
    ] {
        let mut output = chunk.clone();
        output["totalBytes"] = json!(total);
        output["chunkBase64"] = json!(data);
        output["nextOffset"] = next;
        add("artifact.query", output);
    }
    for count in [128, 129] {
        let mut output = page.clone();
        output["artifacts"] = json!(vec![artifact.clone(); count]);
        add("artifact.query", output);
    }
    for count in [32768, 32769] {
        for text in ["a".repeat(count), "\0".repeat(count)] {
            add(
                "artifact.query",
                json!({"kind":"text","sessionId":"session","artifactId":"artifact",
                "preview":{"ok":true,"text":text}}),
            );
        }
    }
    for kind in ["text", "binary"] {
        for preview in [
            json!({"ok":false,"reason":"too_large"}),
            json!({"ok":false,"reason":"unsupported_mime"}),
            json!({"ok":false,"reason":"not_found","text":""}),
            json!({"ok":true,"reason":"too_large"}),
            json!({"ok":0,"reason":"not_found"}),
            json!({"ok":true,"base64":"AB==","mimeType":"image/png"}),
            json!({"ok":true,"text":""}),
        ] {
            add(
                "artifact.query",
                json!({"kind":kind,"sessionId":"session",
                "artifactId":"artifact","preview":preview}),
            );
        }
    }
    let receipt = json!({"kind":"committed","uploadId":"upload","attachment":{
        "kind":"other","name":"x","mimeType":"x","bytes":1.0,
        "ref":{"kind":"session_file","sessionId":"session","relativePath":"folder/file"}}});
    add("artifact.ingest", receipt.clone());
    for path in [
        "../file", "/file", "C:file", "a\\b", "a//b", "a/./b", "a\0b", "a\nb",
    ] {
        let mut output = receipt.clone();
        output["attachment"]["ref"]["relativePath"] = json!(path);
        add("artifact.ingest", output);
    }
    let mut large_receipt = receipt.clone();
    large_receipt["attachment"]["name"] = json!("x".repeat(513));
    large_receipt["attachment"]["bytes"] = json!(50 * 1024 * 1024 + 1);
    add("artifact.ingest", large_receipt);
    let mut foreign_receipt = receipt;
    foreign_receipt["attachment"]["ref"] = json!({"kind":"workspace_file","relativePath":"file"});
    add("artifact.ingest", foreign_receipt);
    for kind in ["upload_opened", "chunk_accepted", "upload_aborted"] {
        let mut output = json!({"kind":kind,"uploadId":"upload"});
        if kind != "upload_aborted" {
            output["nextOffset"] = json!(0.0);
        }
        add("artifact.ingest", output.clone());
        output["extra"] = json!(true);
        add("artifact.ingest", output);
    }
    add(
        "artifact.query",
        json!({"kind":"revision_changed","expected":revision,"actual":revision}),
    );
    add("artifact.delete", json!({"kind":"deleted"}));
    add("artifact.delete", json!({"kind":"deleted","extra":true}));
    source::compare(&cases);
}
