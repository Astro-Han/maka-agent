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

use maka_runtime::terminal::{
    MouseEncoding, MouseTracking, TerminalInputModes, TerminalSize,
    input::{InputAction, encode_actions, encoded_actions_byte_len},
};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn input_matches_original_contract_and_encoder_across_keys_modes_and_rejections() {
    let named = [
        "enter",
        "escape",
        "tab",
        "backspace",
        "delete",
        "arrow_up",
        "arrow_down",
        "arrow_left",
        "arrow_right",
        "home",
        "end",
        "insert",
        "page_up",
        "page_down",
        "f1",
        "f2",
        "f3",
        "f4",
        "f5",
        "f6",
        "f7",
        "f8",
        "f9",
        "f10",
        "f11",
        "f12",
    ];
    let keys = named
        .into_iter()
        .map(String::from)
        .chain((32u8..=126).map(|c| char::from(c).to_string()));
    let mut cases = Vec::new();
    let state = json!({"applicationCursorKeysMode":false,"mouseTrackingMode":"any",
        "mouseEncoding":"sgr","cols":80,"rows":24});
    for key in keys {
        for mask in 0..8 {
            for application in [false, true] {
                let mut live = state.clone();
                live["applicationCursorKeysMode"] = json!(application);
                cases.push(json!({"actions":[{"type":"key","key":key,"modifiers":modifiers(mask)}],"state":live}));
            }
        }
    }
    for tracking in ["none", "x10", "vt200", "drag", "any"] {
        for encoding in ["default", "sgr", "sgr_pixels"] {
            for mask in 0..8 {
                for event in ["click", "press", "release", "move", "scroll"] {
                    for button in [None, Some("left"), Some("middle"), Some("right")] {
                        let mut action = json!({"type":"mouse","event":event,"x":79,"y":23,"modifiers":modifiers(mask)});
                        if let Some(button) = button {
                            action["button"] = json!(button);
                        }
                        if event == "scroll" {
                            action["direction"] = json!("down");
                        }
                        let mut live = state.clone();
                        live["mouseTrackingMode"] = json!(tracking);
                        live["mouseEncoding"] = json!(encoding);
                        cases.push(json!({"actions":[action],"state":live}));
                    }
                }
            }
        }
    }
    for action in [
        json!({"type":"text","text":"中文😀"}),
        json!({"type":"text","text":""}),
        json!({"type":"text","text":"\u{85}"}),
        json!({"type":"text","text":"\u{1b}[31m"}),
        json!({"type":"text","text":"hello","key":null}),
        json!({"type":"key","key":"😀"}),
        json!({"type":"key","key":"c","modifiers":["ctrl","ctrl"]}),
        json!({"type":"key","key":"c","modifiers":null}),
        json!({"type":"key","key":"c","modifiers":["meta"]}),
        json!({"type":"mouse","event":"move","x":1.0,"y":0.0}),
        json!({"type":"mouse","event":"move","x":1,"y":0,"button":null}),
        json!({"type":"mouse","event":"move","x":80,"y":0}),
        json!({"type":"mouse","event":"move","x":1,"y":24}),
        json!({"type":"mouse","event":"move","x":-1,"y":0}),
        json!({"type":"mouse","event":"move","x":1.5,"y":0}),
        json!({"type":"mouse","event":"move","x":9_007_199_254_740_992u64,"y":0}),
        json!({"type":"mouse","event":"scroll","direction":"up","x":0,"y":0}),
    ] {
        cases.push(json!({"actions":[action],"state":state}));
    }
    for actions in [
        json!([]),
        json!(vec![json!({"type":"key","key":"enter"}); 65]),
        json!([{"type":"text","text":"x".repeat(65536)}]),
        json!([{"type":"text","text":"x".repeat(65536)},{"type":"key","key":"enter"}]),
        json!([{"type":"text","text":"中".repeat(21846)}]),
        json!([{"type":"text","text":"valid prefix"},{"type":"key","key":"enter","modifiers":["alt"]}]),
    ] {
        cases.push(json!({"actions":actions,"state":state}));
    }
    let expected = oracle(&cases);
    assert_eq!(expected.len(), cases.len());
    for (index, (case, expected)) in cases.iter().zip(expected).enumerate() {
        let state = &case["state"];
        let modes = TerminalInputModes {
            application_cursor_keys_mode: state["applicationCursorKeysMode"].as_bool().unwrap(),
            mouse_tracking_mode: serde_json::from_value::<MouseTracking>(
                state["mouseTrackingMode"].clone(),
            )
            .unwrap(),
            mouse_encoding: serde_json::from_value::<MouseEncoding>(state["mouseEncoding"].clone())
                .unwrap(),
        };
        let result = case["actions"]
            .as_array()
            .unwrap()
            .iter()
            .cloned()
            .map(InputAction::parse)
            .collect::<Result<Vec<_>, _>>()
            .and_then(|actions| {
                let encoded = encode_actions(&actions, modes, TerminalSize::new(80, 24).unwrap())?;
                let bytes = encoded_actions_byte_len(&actions)?;
                Ok(json!({"encoded":encoded,"bytes":bytes}))
            });
        assert_eq!(
            result.unwrap_or(Value::Null),
            expected,
            "case {index}: {case}"
        );
    }
}

fn modifiers(mask: u8) -> Vec<&'static str> {
    ["shift", "alt", "ctrl"]
        .into_iter()
        .enumerate()
        .filter_map(|(index, name)| (mask & (1 << index) != 0).then_some(name))
        .collect()
}

fn oracle(cases: &[Value]) -> Vec<Value> {
    let mut child = Command::new("node")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/terminal-input-oracle.mjs"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(cases).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
