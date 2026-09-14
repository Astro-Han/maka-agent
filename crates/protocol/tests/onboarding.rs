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

use maka_protocol::onboarding::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn onboarding_wire_matches_source_required_nulls_identity_and_variant_limits() {
    let base = json!({"target":{"kind":"create","providerType":"openrouter"},"apiKey":null,"baseUrl":null});
    let mut cases = Vec::new();
    let mut add = |save, output, value: Value| {
        let result = if output {
            if save {
                decode_save_result(&value).map(|v| json!(v))
            } else {
                decode_verify_result(&value).map(|v| json!(v))
            }
        } else {
            decode_input(&value, save).map(|(input, ids)| {
                let mut v =
                    json!({"target":input.target,"apiKey":input.api_key,"baseUrl":input.base_url});
                if save {
                    v["enabledModelIds"] = json!(ids);
                }
                v
            })
        };
        cases.push(
            json!({"save":save,"output":output,"value":value,"expected":match result {
                Ok(value)=>json!({"ok":true,"value":value}),Err(_)=>json!({"ok":false})
            }}),
        );
    };
    add(false, false, base.clone());
    for field in ["apiKey", "baseUrl", "target"] {
        let mut v = base.clone();
        v.as_object_mut().unwrap().remove(field);
        add(false, false, v);
    }
    for value in [
        json!(""),
        json!(5),
        json!("x".repeat(65537)),
        json!("\u{feff}key\u{feff}"),
    ] {
        let mut v = base.clone();
        v["apiKey"] = value;
        add(false, false, v);
    }
    for target in [
        json!({"kind":"create","providerType":"openrouter","slug":"my-relay","name":""}),
        json!({"kind":"create","providerType":"openrouter","name":null}),
        json!({"kind":"create","providerType":"openrouter","slug":"x"}),
        json!({"kind":"create","providerType":"bogus"}),
        json!({"kind":"existing","connectionId":"existing-id"}),
        json!({"kind":"existing","connectionId":"bad/id"}),
        json!({"kind":"existing","connectionId":"x","providerType":"openrouter"}),
    ] {
        let mut v = base.clone();
        v["target"] = target;
        add(false, false, v);
    }
    for ids in [
        json!([]),
        json!(["one"]),
        json!(["one", "one"]),
        json!([""]),
        json!(vec!["x"; 2049]),
    ] {
        let mut v = base.clone();
        v["enabledModelIds"] = ids;
        add(true, false, v);
    }
    for output in [
        json!({"kind":"verified","models":[{"id":"one","contextWindow":4096}]}),
        json!({"kind":"verified","models":[]}),
        json!({"kind":"failed","errorClass":"network"}),
        json!({"kind":"failed","errorClass":"bogus"}),
        json!({"kind":"rejected","reason":"slug_taken"}),
        json!({"kind":"rejected","reason":"superseded"}),
        json!({"kind":"rejected","reason":"model_unavailable"}),
    ] {
        add(false, true, output);
    }
    for revision in [json!(1), json!(1.0), json!(0), json!(1.5)] {
        add(
            true,
            true,
            json!({"kind":"saved","connection":{"connectionId":"e1f7b7af-a771-4530-b83e-c80a4ac235b4","revision":revision,"slug":"relay","providerType":"openrouter"}}),
        );
    }
    let mut child = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/onboarding_source.mjs"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&cases).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
