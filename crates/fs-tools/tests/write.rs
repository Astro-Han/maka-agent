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
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};
use std::{fs, sync::Arc};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn write_contract_authority_bounds_and_inode() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    // Ordinary fixture paths use the same spelling as Node realpath on Windows.
    #[cfg(windows)]
    let root = std::path::PathBuf::from(root.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
    let coordinator = Arc::new(WriteCoordinator::default());
    let executor = MutationExecutor::new(
        &root,
        WriteScope::Restricted {
            roots: vec![root.clone()],
        },
        coordinator.clone(),
    )
    .unwrap();
    fs::write(root.join("text"), "old content").unwrap();
    #[cfg(unix)]
    fs::set_permissions(root.join("text"), fs::Permissions::from_mode(0o640)).unwrap();
    fs::hard_link(root.join("text"), root.join("hard")).unwrap();
    let before = File::from_std(fs::File::open(root.join("text")).unwrap())
        .metadata()
        .unwrap();
    let result = executor
        .invoke(
            "Write".into(),
            json!({"path":"text","content":"零"}),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        json!({"kind":"file_write","path":root.join("text"),"bytes":3})
    );
    assert_eq!(fs::read(root.join("hard")).unwrap(), "零".as_bytes());
    let after = File::from_std(fs::File::open(root.join("text")).unwrap())
        .metadata()
        .unwrap();
    assert_eq!(
        (before.dev(), before.ino(), before.permissions()),
        (after.dev(), after.ino(), after.permissions())
    );
    executor
        .invoke(
            "Write".into(),
            json!({"path":"new","content":""}),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(fs::read(root.join("new")).unwrap(), b"");
    fs::create_dir(root.join("dir")).unwrap();
    #[cfg(unix)]
    {
        symlink("text", root.join("link")).unwrap();
        symlink("dir", root.join("dir-link")).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(root.join("fifo"))
                .status()
                .unwrap()
                .success()
        );
    }
    for input in [
        json!({"path":"text","content":"x".repeat(1024 * 1024 + 1)}),
        json!({"path":"x".repeat(4097),"content":"x"}),
        json!({"path":"text","content":"x","extra":1}),
        #[cfg(unix)]
        json!({"path":"link","content":"x"}),
        json!({"path":"dir","content":"x"}),
        #[cfg(unix)]
        json!({"path":"dir-link/file","content":"x"}),
        #[cfg(unix)]
        json!({"path":"fifo","content":"x"}),
        json!({"path":"missing/child","content":"x"}),
        json!({"path":"../outside","content":"x"}),
    ] {
        assert!(matches!(
            executor
                .invoke("Write".into(), input, CancellationToken::new())
                .await,
            Err(ToolError::Failed(_))
        ));
    }
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        executor
            .invoke(
                "Write".into(),
                json!({"path":"text","content":"x"}),
                cancelled
            )
            .await
            .is_err()
    );
    let disabled = MutationExecutor::new(&root, WriteScope::Disabled, coordinator.clone()).unwrap();
    assert!(disabled.names().is_empty());
    assert!(
        disabled
            .invoke(
                "Write".into(),
                json!({"path":"text","content":"x"}),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    assert_eq!(fs::read(root.join("text")).unwrap(), "零".as_bytes());
    let outside = tempfile::NamedTempFile::new().unwrap();
    assert!(
        executor
            .invoke(
                "Write".into(),
                json!({"path":outside.path(),"content":"x"}),
                CancellationToken::new()
            )
            .await
            .is_err()
    );
    let bypass = MutationExecutor::new(&root, WriteScope::Unrestricted, coordinator).unwrap();
    // Canonical path avoids macOS's /var alias, since write paths refuse symlinks.
    bypass
        .invoke(
            "Write".into(),
            json!({"path":outside.path().canonicalize().unwrap(),"content":"bypass"}),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(fs::read(outside.path()).unwrap(), b"bypass");
}
