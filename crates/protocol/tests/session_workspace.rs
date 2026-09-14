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

use maka_protocol::session::*;
use serde_json::{Value, json};

#[test]
fn relocation_preserves_workspace_selection_and_exact_revision_contract() {
    let input =
        |workspace| json!({"sessionId":"session","expectedRevision":1,"workspace":workspace});
    for workspace in [
        json!({"kind":"host_path","path":"/alias/../work"}),
        json!({"kind":"project","projectId":"project"}),
    ] {
        let value = input(workspace);
        let decoded = decode_session_workspace_relocate_input(&value).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), value);
        let conflict = SessionUpdateResult::RevisionConflict {
            expected_revision: 1,
            actual_revision: 2,
        };
        assert!(assert_workspace_relocate_output_for_input(&decoded, &conflict).is_ok());
        let other = SessionUpdateResult::RevisionConflict {
            expected_revision: 2,
            actual_revision: 3,
        };
        assert!(assert_workspace_relocate_output_for_input(&decoded, &other).is_err());
    }
    let valid = input(json!({"kind":"host_path","path":"/work"}));
    for field in ["sessionId", "expectedRevision", "workspace"] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(decode_session_workspace_relocate_input(&missing).is_err());
    }
    for (field, invalid) in [
        ("workspace", Value::Null),
        ("workspace", json!({"kind":"host_path","path":"relative"})),
        (
            "workspace",
            json!({"kind":"host_path","path":"/work","projectId":"project"}),
        ),
        ("expectedRevision", json!(0)),
        ("expectedRevision", json!(9_007_199_254_740_992u64)),
        ("sessionId", json!("bad:id")),
        ("patch", json!({})),
    ] {
        let mut value = valid.clone();
        value[field] = invalid;
        assert!(
            decode_session_workspace_relocate_input(&value).is_err(),
            "{value}"
        );
    }
    let mut floating = valid;
    floating["expectedRevision"] = json!(1.0);
    assert_eq!(
        decode_session_workspace_relocate_input(&floating)
            .unwrap()
            .expected_revision,
        1
    );
}
