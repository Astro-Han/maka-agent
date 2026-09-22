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

use maka_fs_tools::{MutationExecutor, ReadExecutor, ReadLimits, WriteCoordinator, directory};
#[cfg(unix)]
use maka_runtime::read::ReadInput;
use maka_runtime::tools::ToolExecutor;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn directory_consent_does_not_mark_workspaces_or_follow_replacements() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("user-files");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("note"), "original").unwrap();
    let (location, identity) = directory::capture(&root).unwrap();
    assert!(!root.join(".maka-workspace.json").exists());
    let handle = directory::open(&location, &identity).unwrap();
    let read = ReadExecutor::from_directory(
        location.clone(),
        handle.try_clone().unwrap(),
        ReadLimits::default(),
        None,
    )
    .unwrap();
    let write = MutationExecutor::from_directory(
        location.clone(),
        handle,
        Arc::new(WriteCoordinator::default()),
        None,
    )
    .unwrap();
    write
        .invoke(
            "Write".into(),
            json!({"path":"note","content":"authorized"}),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        directory::capture(&root).unwrap().1,
        identity,
        "normal writes do not revoke consent"
    );
    let moved = temp.path().join("original-directory");
    #[cfg(unix)]
    {
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("note"), "replacement").unwrap();
        assert!(directory::open(&location, &identity).is_err());
        let page = read
            .read(
                ReadInput {
                    path: "note".into(),
                    offset: None,
                    limit: None,
                }
                .resolve()
                .unwrap(),
                Default::default(),
            )
            .await
            .unwrap();
        let maka_fs_tools::ReadOutput::Text(page) = page else {
            panic!("text file expected")
        };
        assert_eq!(page.content.trim(), "authorized");
        write
            .invoke(
                "Write".into(),
                json!({"path":"note","content":"settled"}),
                Default::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(moved.join("note")).unwrap(),
            "settled"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("note")).unwrap(),
            "replacement"
        );
    }
    #[cfg(windows)]
    {
        assert!(
            std::fs::rename(&root, &moved).is_err(),
            "captured directories deny FILE_SHARE_DELETE"
        );
        drop(read);
        drop(write);
        std::fs::rename(&root, &moved).unwrap();
        std::fs::create_dir(&root).unwrap();
        assert!(directory::open(&location, &identity).is_err());
    }
}
