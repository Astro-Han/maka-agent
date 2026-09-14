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

pub fn cases(add: &mut impl FnMut(&str, Value)) -> Value {
    let form = json!({"kind":"form","toolUseId":"tool","message":"Line\n",
        "requester":{"name":"Tool"},"fields":[{"kind":"integer","name":"n",
        "label":"N","required":true,"minimum":1,"maximum":3}]});
    add("request", form.clone());
    for action in ["accept", "decline", "cancel"] {
        let mut answer = json!({"kind":"form","action":action});
        if action == "accept" {
            answer["values"] = json!({"n":2});
        }
        add("answer", answer.clone());
        let mut outcome = answer;
        outcome["kind"] = json!("form_answer");
        outcome["committedAt"] = json!(1.0);
        add("outcome", outcome.clone());
        let mut snapshot = super::pending();
        snapshot["request"] = form.clone();
        snapshot["revision"] = json!(2);
        snapshot["status"] = json!("answered");
        snapshot["outcome"] = outcome;
        add("answered", snapshot);
    }
    for padding in [8050, 8070, 8090] {
        let values = json!({"a":"x".repeat(2000),"b":"x".repeat(2000),
            "c":"x".repeat(2000),"d":"x".repeat(2000),"e":"x".repeat(padding-8000),"number":1.0});
        add(
            "answer",
            json!({"kind":"form","action":"accept","values":values}),
        );
        add(
            "outcome",
            json!({"kind":"form_answer","action":"accept","values":values,"committedAt":1}),
        );
    }
    form
}
