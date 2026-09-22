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

use maka_plugins::filesystem::entries::{self, Operation, ReadFile, WriteFile};
use maka_sandbox::filesystem::{Access, Policy, Rule};
use std::fs;

#[test]
fn raw_files_cannot_bypass_metadata_denials_with_bytes_or_directory_renames() {
    let temp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(temp.path()).unwrap();
    for name in ["protected", "private", "source", "target"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    fs::write(root.join("private/key"), "secret").unwrap();
    fs::write(root.join("source/file"), "ordinary").unwrap();
    fs::hard_link(root.join("private/key"), root.join("linked")).unwrap();
    let policy = Policy {
        default: Access::Write,
        rules: vec![
            Rule::subtree(root.join("protected"), Access::Read),
            Rule::subtree(root.join("private"), Access::Deny),
            Rule::subtree(root.join("source/future"), Access::Read),
        ],
        deny_globs: vec![],
    }
    .compile()
    .unwrap();
    let directory =
        cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
    let execute = |operation| {
        entries::execute(
            &directory,
            operation,
            &Default::default(),
            Some(entries::Policy {
                root: &root,
                filesystem: &policy,
            }),
        )
    };
    let write = |path: &str| {
        Operation::Write(WriteFile {
            path: path.into(),
            offset: 0,
            bytes: b"changed".to_vec(),
            truncate: true,
            create_new: false,
            mode: None,
        })
    };
    for operation in [
        write("protected/new"),
        write("linked"),
        Operation::CreateDirectory {
            path: "protected/new".into(),
        },
        Operation::Remove {
            path: "protected".into(),
        },
        Operation::Rename {
            from: "source".into(),
            to: "moved".into(),
        },
        Operation::Rename {
            from: "target".into(),
            to: "protected/new".into(),
        },
        Operation::Read(ReadFile {
            path: "private/key".into(),
            offset: 0,
            limit: 100,
        }),
    ] {
        assert!(execute(operation).is_err());
    }
    execute(write("target/normal")).unwrap();
    assert!(!root.join("protected/new").exists());
    assert!(!root.join("moved").exists());
    assert_eq!(
        fs::read_to_string(root.join("private/key")).unwrap(),
        "secret"
    );
    assert_eq!(
        fs::read_to_string(root.join("target/normal")).unwrap(),
        "changed"
    );
}
