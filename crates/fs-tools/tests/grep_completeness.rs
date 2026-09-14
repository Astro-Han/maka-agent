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
use std::fs;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn exact_caps_and_unscanned_paths_have_explicit_completeness() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let reader = ReadExecutor::new(
        &root,
        ReadScope::Restricted {
            roots: vec![root.clone()],
        },
        ReadLimits::default(),
    )
    .unwrap();
    for (lines, complete) in [
        (0, true),
        (49, true),
        (50, true),
        (51, false),
        (1187, false),
    ] {
        fs::write(root.join("single.txt"), "match\n".repeat(lines)).unwrap();
        let result = reader
            .invoke(
                "Grep".into(),
                json!({"pattern":"match","path":"single.txt"}),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result["matches"].as_array().unwrap().len(), lines.min(50));
        assert_eq!(result["complete"], complete, "{lines} matching lines");
    }
    fs::create_dir(root.join("directory")).unwrap();
    for index in 0..4 {
        fs::write(
            root.join(format!("directory/{index}.txt")),
            "match\n".repeat(50),
        )
        .unwrap();
    }
    // Ignored files are outside the search scope, not unscanned eligible work.
    fs::write(root.join("directory/.ignore"), "ignored.txt\n").unwrap();
    fs::write(root.join("directory/ignored.txt"), "match").unwrap();
    let args = json!({"pattern":"match","path":"directory"});
    let exact = reader
        .invoke("Grep".into(), args.clone(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(exact["matches"].as_array().unwrap().len(), 200);
    assert_eq!(exact["complete"], true);
    fs::create_dir(root.join("directory/z-later")).unwrap();
    fs::write(root.join("directory/z-later/empty.txt"), "").unwrap();
    let unscanned = reader
        .invoke("Grep".into(), args, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(unscanned["matches"], exact["matches"]);
    assert_eq!(
        unscanned["complete"], false,
        "no claim about an unscanned directory, even if it happens to be empty"
    );
    let narrowed = reader
        .invoke(
            "Grep".into(),
            json!({"pattern":"match","path":"directory/z-later"}),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(narrowed, json!({"matches":[],"complete":true}));
}
