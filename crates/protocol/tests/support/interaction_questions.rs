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

use serde_json::{Value, json};

pub fn request() -> Value {
    json!({"kind":"question","toolUseId":"tool","questions":[
        {"question":"Choose\n", "options":[{"label":"Yes"},{"label":"No","description":"Decline"}]}
    ]})
}

pub fn cases(add: &mut impl FnMut(&str, Value)) {
    let request = request();
    add("request", request.clone());
    for (pointer, values) in [
        (
            "/questions",
            vec![json!([]), json!(vec![request["questions"][0].clone(); 4])],
        ),
        (
            "/questions/0/question",
            vec![json!(""), json!("😀".repeat(256)), json!("😀".repeat(257))],
        ),
        (
            "/questions/0/options",
            vec![
                json!([]),
                json!([{"label":"A"}]),
                json!([{"label":"A"},{"label":"A"}]),
            ],
        ),
        (
            "/questions/0/options/1/label",
            vec![json!(""), json!("😀".repeat(64)), json!("😀".repeat(65))],
        ),
        (
            "/questions/0/options/1/description",
            vec![
                Value::Null,
                json!(""),
                json!("😀".repeat(128)),
                json!("😀".repeat(129)),
            ],
        ),
    ] {
        for value in values {
            let mut mutated = request.clone();
            *mutated.pointer_mut(pointer).unwrap() = value;
            add("request", mutated);
        }
    }
    for pointer in ["/questions/0", "/questions/0/options/0"] {
        let mut mutated = request.clone();
        mutated.pointer_mut(pointer).unwrap()["extra"] = json!(1);
        add("request", mutated);
    }
    for answers in [
        json!([]),
        json!([null]),
        json!(["free text"]),
        json!([" "]),
        json!([""]),
        json!([null, null, null]),
        json!([null, null, null, null]),
        json!([false]),
        json!(["😀".repeat(512)]),
        json!(["😀".repeat(513)]),
        json!([null, "free text", "Yes"]),
    ] {
        add("answer", json!({"kind":"question","answers":answers}));
        let outcome = json!({"kind":"question_answer","answers":answers,"committedAt":1});
        add("outcome", outcome.clone());
        let mut snapshot = super::pending();
        snapshot["request"] = request.clone();
        snapshot["outcome"] = outcome;
        snapshot["revision"] = json!(2);
        snapshot["status"] = json!("answered");
        add("snapshot", snapshot.clone());
        add("answered", snapshot);
    }
    // Escaping makes the encoded budget larger than the individual text byte sums.
    for padding in [1300, 1345, 1355] {
        let answers = json!([
            "\n".repeat(padding),
            "\n".repeat(padding),
            "\n".repeat(padding)
        ]);
        add("answer", json!({"kind":"question","answers":answers}));
        add(
            "outcome",
            json!({"kind":"question_answer","answers":answers,"committedAt":9_007_199_254_740_991_u64}),
        );
    }
    for reason in ["timed_out", "turn_stopped", "host_restarted"] {
        let mut snapshot = super::pending();
        snapshot["request"] = request.clone();
        snapshot["revision"] = json!(2);
        snapshot["status"] = json!("closed");
        snapshot["outcome"] = json!({"kind":"closure","reason":reason,"committedAt":1});
        add("snapshot", snapshot);
    }
}
