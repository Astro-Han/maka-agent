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

use cap_fs_ext::MetadataExt;
use cap_std::fs::File;
use maka_fs_tools::{MutationExecutor, WriteCoordinator, WriteScope};
use maka_runtime::tools::{ToolError, ToolExecutor};
use serde_json::{Value, json};
use std::{fs, sync::Arc};
use tokio_util::sync::CancellationToken;

fn executor(root: &std::path::Path) -> MutationExecutor {
    MutationExecutor::new(
        root,
        WriteScope::Restricted {
            roots: vec![root.to_owned()],
        },
        Arc::new(WriteCoordinator::default()),
    )
    .unwrap()
}
async fn edit(executor: &MutationExecutor, input: Value) -> Result<Value, ToolError> {
    executor
        .invoke("Edit".into(), input, CancellationToken::new())
        .await
}

#[tokio::test]
async fn exact_literal_replacement_preserves_inode_and_reports_original_line_span() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    #[cfg(windows)]
    let root = std::path::PathBuf::from(root.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
    let executor = executor(&root);
    let path = root.join("text");
    fs::write(&path, "零\n一\n二\n三\n").unwrap();
    fs::hard_link(&path, root.join("alias")).unwrap();
    let original = File::from_std(fs::File::open(&path).unwrap())
        .metadata()
        .unwrap();
    let result = edit(
        &executor,
        json!({"path":"text","old_string":"一\n二\n","new_string":"$&$1\\n"}),
    )
    .await
    .unwrap();
    assert_eq!(
        result,
        json!({"ok":true,"path":path,"replacements":1,"matchedVia":"exact","startLine":2,"endLine":3})
    );
    assert_eq!(
        fs::read_to_string(root.join("alias")).unwrap(),
        "零\n$&$1\\n三\n"
    );
    let after = File::from_std(fs::File::open(&path).unwrap())
        .metadata()
        .unwrap();
    assert_eq!(
        (after.dev(), after.ino(), after.permissions()),
        (original.dev(), original.ino(), original.permissions())
    );
    fs::write(&path, "aaa").unwrap();
    // The second overlapping "aa" is not a second non-overlapping occurrence.
    edit(
        &executor,
        json!({"path":"text","old_string":"aa","new_string":""}),
    )
    .await
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"a");
    fs::write(&path, "a\nb").unwrap();
    assert_eq!(
        edit(
            &executor,
            json!({"path":"text","old_string":"\n","new_string":"X"})
        )
        .await
        .unwrap()["endLine"],
        1
    );
    assert_eq!(fs::read(&path).unwrap(), b"aXb");
}

#[tokio::test]
async fn invalid_ambiguous_missing_and_oversized_edits_leave_bytes_intact() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let executor = executor(&root);
    let path = root.join("text");
    let cases = [
        (
            b"one one".to_vec(),
            json!({"path":"text","old_string":"one","new_string":"two"}),
        ),
        (
            b"one".to_vec(),
            json!({"path":"text","old_string":"","new_string":"two"}),
        ),
        (
            b"one".to_vec(),
            json!({"path":"text","old_string":"one","new_string":"one"}),
        ),
        (
            b"  one".to_vec(),
            json!({"path":"text","old_string":"one  ","new_string":"two"}),
        ),
        (
            vec![0xff, b'x'],
            json!({"path":"text","old_string":"x","new_string":"two"}),
        ),
        (
            b"xy".to_vec(),
            json!({"path":"text","old_string":"x","new_string":"z".repeat(1024 * 1024)}),
        ),
        (
            vec![b'x'; 1024 * 1024 + 1],
            json!({"path":"text","old_string":"x","new_string":"two"}),
        ),
        (
            b"one".to_vec(),
            json!({"path":"text","old_string":"one","new_string":"two","extra":true}),
        ),
        (
            b"one".to_vec(),
            json!({"path":"text","old_string":"x".repeat(1024 * 1024 + 1),"new_string":"two"}),
        ),
    ];
    for (source, input) in cases {
        fs::write(&path, &source).unwrap();
        assert!(matches!(
            edit(&executor, input).await,
            Err(ToolError::Failed(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), source);
    }
    assert!(
        edit(
            &executor,
            json!({"path":"missing","old_string":"one","new_string":"two"})
        )
        .await
        .is_err()
    );
    assert!(!root.join("missing").exists());
    fs::create_dir(root.join("dir")).unwrap();
    assert!(
        edit(
            &executor,
            json!({"path":"dir","old_string":"one","new_string":"two"})
        )
        .await
        .is_err()
    );
    let denied = MutationExecutor::new(
        &root,
        WriteScope::Disabled,
        Arc::new(WriteCoordinator::default()),
    )
    .unwrap();
    assert!(
        edit(
            &denied,
            json!({"path":"text","old_string":"one","new_string":"two"})
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"one");
}
