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

use maka_fs_tools::{ReadExecutor, ReadLimits, ReadScope};
use maka_runtime::tools::ToolExecutor;
use serde_json::json;
use std::{fs, process::Command};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn slash_glob_uses_session_cwd_independently_of_the_granted_root() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let cwd = root.join("app");
    fs::create_dir_all(cwd.join("src")).unwrap();
    fs::write(cwd.join("src/main.rs"), "token\n").unwrap();
    let output = Command::new("rg")
        .args([
            "--no-config",
            "-n",
            "--no-heading",
            "--color=never",
            "--glob",
            "src/*.rs",
            "--",
            "token",
        ])
        .arg(dunce::simplified(&cwd))
        .current_dir(&cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Vec<_> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(expected.len(), 1);
    for roots in [vec![root.clone()], vec![root.clone(), cwd.clone()]] {
        let executor =
            ReadExecutor::new(&cwd, ReadScope::Restricted { roots }, ReadLimits::default())
                .unwrap();
        for path in [".", "src/../src"] {
            let result = executor
                .invoke(
                    "Grep".into(),
                    json!({"pattern":"token","glob":"src/*.rs","path":path}),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(
                result,
                json!({"matches":expected,"complete":true}),
                "path={path}"
            );
        }
    }
}
