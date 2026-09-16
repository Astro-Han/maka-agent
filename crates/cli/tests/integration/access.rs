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

use super::candidate::CandidateFixture;
use std::{
    path::Path,
    process::{Command, Stdio},
};

#[test]
fn native_pairing_finalizes_once_binds_client_and_revokes_live_transport() {
    let temporary = tempfile::tempdir().unwrap();
    let invalid_root = temporary.path().join("invalid");
    let invalid = Command::new(env!("CARGO_BIN_EXE_maka"))
        .args(["host", "setup", "--root"])
        .arg(&invalid_root)
        .args(["--principal", ""])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(
        !invalid_root.exists(),
        "invalid pairing must not install a Host"
    );
    let mut fixture = CandidateFixture::new(temporary.path().join("root"));
    let installed = Command::new(env!("CARGO_BIN_EXE_maka"))
        .args(["host", "install", "--root"])
        .arg(&fixture.root)
        .output()
        .unwrap();
    assert!(installed.status.success(), "{installed:?}");
    let deployment: serde_json::Value = serde_json::from_slice(&installed.stdout).unwrap();
    let executable = deployment["executable"].as_str().unwrap();
    fixture.child = Some(
        // A foreground candidate also works inside Windows Cargo's enclosing Job.
        Command::new(executable)
            .args(["host", "candidate", "--root"])
            .arg(&fixture.root)
            .args([
                "--expected-root-id",
                &fixture.root_id,
                "--startup-attempt-id",
                &uuid::Uuid::new_v4().to_string(),
                "--owner-stdin",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    fixture.wait_for_registration();
    let client = Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs"))
        .args(["--native-access", executable, "--root"])
        .arg(&fixture.root)
        .output()
        .unwrap();
    assert!(
        client.status.success(),
        "{}",
        String::from_utf8_lossy(&client.stderr)
    );
    assert!(
        std::fs::read_dir(fixture.registration.parent().unwrap())
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("runtime-host-access-delivery-"))
    );
}
