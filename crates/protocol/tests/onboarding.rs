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
fn onboarding_wire_matches_source_provider_identity_and_variant_limits() {
    let provider = json!({"packageId":"example.provider","entryId":"example.provider","scope":"profile","name":"custom"});
    let target = json!({"kind":"create","provider":provider,"configuration":{"region":"local"},"slug":"custom","name":"Custom"});
    let base = json!({"target":target});
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
                let mut v = json!({"target":input.target});
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
    add(false, false, json!({}));
    for field in ["apiKey", "baseUrl"] {
        let mut v = base.clone();
        v[field] = Value::Null;
        add(false, false, v);
    }
    for field in ["provider", "configuration", "slug", "name"] {
        let mut v = base.clone();
        v["target"].as_object_mut().unwrap().remove(field);
        add(false, false, v);
    }
    let expected = json!({"connectionId":"existing-id","revision":1,"slug":"custom","provider":provider,"configuration":{"region":"local"}});
    for target in [
        json!({"kind":"create","provider":provider,"configuration":{},"slug":"custom","name":""}),
        json!({"kind":"create","provider":provider,"configuration":[],"slug":"custom","name":"Custom"}),
        json!({"kind":"existing","expected":expected,"configuration":{}}),
        json!({"kind":"existing","connectionId":"existing-id"}),
        json!({"kind":"existing","expected":expected,"configuration":{},"providerType":"openrouter"}),
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
            json!({"kind":"saved","connection":{"connectionId":"e1f7b7af-a771-4530-b83e-c80a4ac235b4","revision":revision,"slug":"relay","provider":provider}}),
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
