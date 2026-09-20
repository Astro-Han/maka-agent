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

use std::collections::HashSet;
use std::path::Path;

use maka_protocol::Operation;

#[test]
fn every_operation_matches_current_typescript_remote_owner_grants() {
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/runtime-host/src/protocol/operations.ts"),
    )
    .expect("Read current worktree TypeScript authority policy");
    let list = source
        .split_once("export const REMOTE_OWNER_OPERATION_GRANTS = Object.freeze([")
        .expect("Find explicit remote owner allowlist")
        .1
        .split_once("] as const satisfies readonly OperationKey[]);")
        .expect("Find end of explicit remote owner allowlist")
        .0;
    let mut grants = HashSet::new();
    for line in list.lines().map(str::trim).filter(|line| !line.is_empty()) {
        // Reject policy syntax changes instead of silently skipping entries.
        let name = line
            .strip_prefix('\'')
            .and_then(|line| line.strip_suffix("',"))
            .expect("Remote owner grant must remain an explicit string entry");
        assert!(grants.insert(name), "Duplicate remote owner grant: {name}");
    }
    assert!(!grants.is_empty(), "Authority policy cannot be empty");
    for &operation in Operation::ALL {
        assert_eq!(
            operation.allows_remote_owner(),
            grants.contains(operation.as_str()),
            "Remote owner policy drift for {operation}"
        );
    }
}
