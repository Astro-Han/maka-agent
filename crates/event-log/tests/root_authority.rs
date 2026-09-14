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

use maka_event_log::root::{ROOT_MARKER, RUST_ROOT_MARKER, RootNamespaces, RootOwner};
use std::{fs, path::Path, process::Command};

fn namespaces(base: &Path) -> RootNamespaces {
    RootNamespaces {
        ownership: base.join("owners"),
        control: base.join("control"),
    }
}

#[test]
#[cfg(unix)]
fn identity_and_lease_survive_reopen_and_symlink_alias() {
    let temp = tempfile::tempdir().unwrap();
    let ns = namespaces(temp.path());
    let path = temp.path().join("root");
    let owner = RootOwner::create(&path, &ns).unwrap();
    let id = owner.root_id().to_owned();
    assert_eq!(id.len(), 64);
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(path.join(ROOT_MARKER)).unwrap()).unwrap();
    assert_eq!(marker["schemaVersion"], 1);
    assert_eq!(marker["kind"], "interactive");
    assert_eq!(marker["rootId"], id);
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert!(RootOwner::open(&alias, &ns).is_err());
    owner.validate_current().unwrap();
    drop(owner);
    assert_eq!(RootOwner::open(&alias, &ns).unwrap().root_id(), id);
}

#[test]
fn copied_marker_and_rebound_path_cannot_reuse_identity() {
    let temp = tempfile::tempdir().unwrap();
    let ns = namespaces(temp.path());
    let path = temp.path().join("root");
    let owner = RootOwner::create(&path, &ns).unwrap();
    let copy = temp.path().join("copy");
    fs::create_dir(&copy).unwrap();
    for name in [ROOT_MARKER, RUST_ROOT_MARKER] {
        fs::copy(path.join(name), copy.join(name)).unwrap();
    }
    assert!(RootOwner::open(&copy, &ns).is_err());
    fs::rename(&path, temp.path().join("moved")).unwrap();
    fs::rename(&copy, &path).unwrap();
    assert!(owner.validate_current().is_err());
}

#[test]
#[cfg(unix)]
fn legacy_and_unsafe_markers_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let ns = namespaces(temp.path());
    let legacy = temp.path().join("legacy");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("runtime.sqlite"), b"legacy").unwrap();
    assert!(RootOwner::create(&legacy, &ns).is_err());
    assert!(!legacy.join(ROOT_MARKER).exists());
    assert!(RootOwner::open(&legacy, &ns).is_err());
    let path = temp.path().join("root");
    let owner = RootOwner::create(&path, &ns).unwrap();
    let skills = temp.path().join("skills");
    fs::create_dir(&skills).unwrap();
    std::os::unix::fs::symlink(&skills, path.join("skills")).unwrap();
    assert!(
        maka_event_log::root::initialize(&path, &ns).is_err(),
        "a Skill directory alias cannot bypass root layout checks"
    );
    fs::remove_file(path.join("skills")).unwrap();
    let saved = temp.path().join("marker");
    fs::rename(path.join(ROOT_MARKER), &saved).unwrap();
    std::os::unix::fs::symlink(&saved, path.join(ROOT_MARKER)).unwrap();
    assert!(owner.validate_current().is_err());
    fs::remove_file(path.join(ROOT_MARKER)).unwrap();
    fs::write(path.join(ROOT_MARKER), vec![b' '; 1025]).unwrap();
    assert!(owner.validate_current().is_err());
}

#[test]
fn durable_lease_blocks_other_process_even_after_cache_removal() {
    if let Some(base) = std::env::var_os("MAKA_ROOT_AUTHORITY_TEST_BASE") {
        let base = Path::new(&base);
        let error = RootOwner::open(&base.join("root"), &namespaces(base))
            .err()
            .unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let ns = namespaces(temp.path());
    let path = temp.path().join("root");
    let owner = RootOwner::create(&path, &ns).unwrap();
    fs::create_dir(path.join("skills")).unwrap();
    assert_eq!(
        maka_event_log::root::initialize(&path, &ns).unwrap(),
        owner.root_id()
    );
    fs::remove_dir_all(&ns.control).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "durable_lease_blocks_other_process_even_after_cache_removal",
            "--nocapture",
        ])
        .env("MAKA_ROOT_AUTHORITY_TEST_BASE", temp.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(owner.validate_current().is_err());
    drop(owner);
    RootOwner::open(&path, &ns).unwrap();
}
