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

pub(super) fn input() -> Value {
    let text = json!({"type":"string","minLength":1,"maxLength":48000});
    let id = json!({"type":"string","minLength":1,"maxLength":128});
    let set = json!({"type":"string","minLength":1,"maxLength":96});
    let title = json!({"type":"string","minLength":1,"maxLength":512});
    let target = json!({"oneOf":[
        {"type":"object","properties":{"disposition":{"const":"delegate_existing"},"candidateRef":id},"required":["disposition","candidateRef"],"additionalProperties":false},
        {"type":"object","properties":{"disposition":{"const":"create_new"},"title":title},"required":["disposition","title"],"additionalProperties":false}
    ]});
    let operations = [
        ("candidates", "Discover fresh candidate references and exact previous delegation IDs. Never invent identities.", json!({}), vec![]),
        ("delegate_existing", "Delegate actual instructions to a candidate from the same fresh set. Admission is not task completion.", json!({"candidateSetId":set,"candidateRef":id,"text":text}), vec!["candidateSetId","candidateRef","text"]),
        ("create_new", "Create a task only when explicitly requested, using this Desktop window's selected workspace and preferences.", json!({"title":title,"text":text}), vec!["title","text"]),
        ("select_and_delegate", "Offer candidates when the existing target is ambiguous. Host records the user's choice and delegates directly; do not delegate again.", json!({"candidateSetId":set,"candidateRefs":{"type":"array","items":id,"minItems":1,"maxItems":32},"text":text}), vec!["candidateSetId","candidateRefs","text"]),
        ("correct", "Correct the exact previous delegation. Existing replacements require fresh candidate references; new work still requires explicit user intent.", json!({"replacesActionId":id,"candidateSetId":set,"target":target,"text":text}), vec!["replacesActionId","target","text"]),
        ("stop", "Stop only the WorkHub-owned delegation for the exact target Session; never unrelated later work.", json!({"targetSessionId":id}), vec!["targetSessionId"]),
        ("resume", "Resume a stopped delegation using its exact Session and previous action ID.", json!({"targetSessionId":id,"resumesActionId":id}), vec!["targetSessionId","resumesActionId"]),
    ].into_iter().map(|(name, description, mut properties, mut required)| {
        properties["operation"] = json!({"const":name});
        required.push("operation");
        json!({"type":"object","description":description,"properties":properties,"required":required,"additionalProperties":false})
    }).collect::<Vec<_>>();
    json!({"type":"object","properties":{"request":{"oneOf":operations}},"required":["request"],"additionalProperties":false})
}
