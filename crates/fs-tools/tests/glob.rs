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
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};
use tokio_util::sync::CancellationToken;

fn reader(root: &Path) -> ReadExecutor {
    ReadExecutor::new(
        root,
        ReadScope::Restricted {
            roots: vec![root.to_owned()],
        },
        ReadLimits::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn glob_matches_node_paths_hidden_names_directory_suffix_and_result_cap() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("src/deep")).unwrap();
    fs::create_dir_all(root.join(".hidden")).unwrap();
    for (path, text) in [
        ("src/a.ts", "a"),
        ("src/B.ts", "b"),
        ("src/deep/z.txt", "z"),
        ("src/.local", "hidden"),
        (".hidden/file.txt", "hidden"),
        ("root.txt", "r"),
    ] {
        fs::write(root.join(path), text).unwrap();
    }
    let executor = reader(&root);
    let mut cases = Vec::new();
    for pattern in [
        "*",
        "**",
        "**/*.ts",
        "src/?.ts",
        "src/[a-z].ts",
        "src/*.TS",
        "src/b.ts",
        "**/.local",
        ".hidden/*",
        "*/",
        "src/**/",
        "missing/*",
        ".",
        "./",
        "./src/*.ts",
    ] {
        let result = executor
            .invoke(
                "Glob".into(),
                json!({"pattern":pattern}),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        cases.push(json!({"pattern":pattern,"files":result["files"]}));
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/glob_source.mjs");
    let mut child = Command::new("node")
        .arg(script)
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), &cases).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for n in 0..210 {
        fs::write(root.join(format!("cap{n:03}.txt")), "").unwrap();
    }
    let result = executor
        .invoke(
            "Glob".into(),
            json!({"pattern":"cap*.txt"}),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let expected: Vec<String> = (0..200).map(|n| format!("cap{n:03}.txt")).collect();
    assert_eq!(result, json!({"files":expected}));
}

#[tokio::test]
#[cfg(unix)]
async fn glob_keeps_captured_authority_and_cannot_walk_external_aliases_or_hide_failures() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("root");
    fs::create_dir(&root).unwrap();
    fs::create_dir(base.join("outside")).unwrap();
    fs::write(base.join("outside/secret.txt"), "").unwrap();
    fs::write(root.join("inside.txt"), "").unwrap();
    symlink(base.join("outside"), root.join("escape")).unwrap();
    let executor = reader(&root);
    fs::rename(&root, base.join("captured")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("replacement.txt"), "").unwrap();
    assert_eq!(
        executor
            .invoke(
                "Glob".into(),
                json!({"pattern":"**/*.txt"}),
                CancellationToken::new()
            )
            .await
            .unwrap(),
        json!({"files":["inside.txt"]})
    );
    for input in [
        json!({"pattern":"**","cwd":"escape"}),
        json!({"pattern":"*","cwd":base.join("outside")}),
        json!({"pattern":"../*"}),
        json!({"pattern":"*","cwd":null}),
        json!({"pattern":"*","cwd":"inside.txt"}),
        json!({"pattern":"[bad"}),
        json!({"pattern":"{a,b}"}),
        json!({"pattern":"!(a)"}),
    ] {
        assert!(
            executor
                .invoke("Glob".into(), input, CancellationToken::new())
                .await
                .is_err()
        );
    }
    let token = CancellationToken::new();
    token.cancel();
    assert!(
        executor
            .invoke("Glob".into(), json!({"pattern":"*"}), token)
            .await
            .is_err()
    );
    let missing = executor
        .invoke(
            "Glob".into(),
            json!({"pattern":"no-such-name"}),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(missing, json!({"files":[]}));
}
