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

use std::{path::Path, process::Command};

#[test]
fn npm_package_round_trips_native_code_offline_and_never_overwrites_a_release() {
    let temporary = tempfile::tempdir().unwrap();
    let executable = env!("CARGO_BIN_EXE_maka");
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("linux", "aarch64") => "linux-arm64-gnu",
        ("linux", "x86_64") => "linux-x64-gnu",
        ("windows", "x86_64") => "win32-x64",
        other => panic!("unsupported release platform: {other:?}"),
    };
    let notices = temporary.path().join("notices.txt");
    std::fs::write(
        &notices,
        "Test fixture only, not a publishable license closure.\n",
    )
    .unwrap();
    let output = temporary.path().join("output");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/rust/pack-cli.mjs");
    let pack = || {
        Command::new("node")
            .arg(&script)
            .args([
                "--target",
                target,
                "--version",
                "0.0.0-test",
                "--binary",
                executable,
                "--validator",
                executable,
                "--notices",
            ])
            .arg(&notices)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap()
    };
    let packed = pack();
    assert!(
        packed.status.success(),
        "{}",
        String::from_utf8_lossy(&packed.stderr)
    );
    let package: serde_json::Value = serde_json::from_slice(&packed.stdout).unwrap();
    let archive = package["archive"].as_str().unwrap();
    let integrity = package["integrity"].as_str().unwrap();
    let cache = temporary.path().join("cache");
    let fetch = Command::new(executable)
        .args([
            "host",
            "fetch",
            "--target",
            target,
            "--version",
            "0.0.0-test",
            "--archive",
            archive,
            "--integrity",
            integrity,
            "--cache",
        ])
        .arg(&cache)
        .output()
        .unwrap();
    assert!(
        fetch.status.success(),
        "{}",
        String::from_utf8_lossy(&fetch.stderr)
    );
    let artifact: serde_json::Value = serde_json::from_slice(&fetch.stdout).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(Path::new(artifact["directory"].as_str().unwrap()).join("package.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["publishConfig"]["tag"], "rust-preview");
    // Bootstrap from transferred bytes, then remove that source. The retained
    // operator must still have its complete package, including Windows service code.
    use sha2::Digest;
    let receipt =
        std::fs::read(Path::new(artifact["directory"].as_str().unwrap()).join("receipt.json"))
            .unwrap();
    let retained = Command::new(artifact["executable"].as_str().unwrap())
        .args([
            "host",
            "fetch",
            "--target",
            target,
            "--version",
            "0.0.0-test",
            "--directory",
            artifact["directory"].as_str().unwrap(),
            "--receipt-sha256",
            &format!("{:x}", sha2::Sha256::digest(receipt)),
            "--framed",
            "--cache",
        ])
        .arg(temporary.path().join("retained"))
        .output()
        .unwrap();
    assert!(
        retained.status.success(),
        "{}",
        String::from_utf8_lossy(&retained.stderr)
    );
    let retained = String::from_utf8(retained.stdout).unwrap();
    let artifact: serde_json::Value = serde_json::from_str(
        retained
            .strip_prefix("__MAKA_NATIVE_HOST_ARTIFACT__")
            .unwrap(),
    )
    .unwrap();
    std::fs::remove_dir_all(&cache).unwrap();
    let imported = artifact["executable"].as_str().unwrap();
    let help = Command::new(imported)
        .args(["host", "setup", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success(), "{help:?}");
    assert!(String::from_utf8_lossy(&help.stdout).contains("--principal"));
    if cfg!(windows) {
        assert!(Path::new(artifact["serviceExecutable"].as_str().unwrap()).is_file());
    }
    // Existing release outputs are immutable, even when a publisher reruns a build.
    let before = std::fs::metadata(archive).unwrap();
    let duplicate = pack();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("EEXIST"));
    assert_eq!(
        std::fs::metadata(archive).unwrap().modified().unwrap(),
        before.modified().unwrap()
    );
}
