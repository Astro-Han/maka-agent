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
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use maka_event_log::root::{RootNamespaces, RootOwner};
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
fn managed_installation_pins_code_before_migration_and_preserves_live_authority() {
    let temporary = tempfile::tempdir().unwrap();
    let namespaces = RootNamespaces::for_current_account().unwrap();
    for mode in ["on-demand", "supervised"] {
        let mut fixture = CandidateFixture::new(temporary.path().join(mode));
        let mut install = Command::new(env!("CARGO_BIN_EXE_maka"));
        install
            .args(["host", "install", "--root"])
            .arg(&fixture.root)
            .args(["--mode", mode]);
        let owner = RootOwner::open(&fixture.root, &namespaces).unwrap();
        assert!(!install.output().unwrap().status.success());
        let directory = namespaces
            .ownership
            .parent()
            .unwrap()
            .join("deployments")
            .join(&fixture.root_id);
        assert!(
            !directory.join("deployment.sqlite").exists(),
            "failed staging must not fence unmanaged startup"
        );
        drop(owner);
        let output = install.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let installed: Value = serde_json::from_slice(&output.stdout).unwrap();
        let executable = installed["executable"].as_str().unwrap();
        assert_eq!(installed["rootId"], fixture.root_id);
        assert_eq!(installed["configRevision"], 1);

        let launch_root = fixture.root.clone();
        let launch_root_id = fixture.root_id.clone();
        let command = |executable: &str, candidate: bool| {
            let mut command = Command::new(executable);
            command
                .args([
                    "host",
                    if candidate {
                        "candidate"
                    } else {
                        "service-run"
                    },
                    "--root",
                ])
                .arg(&launch_root);
            if candidate {
                command.args([
                    "--expected-root-id",
                    &launch_root_id,
                    "--startup-attempt-id",
                    &uuid::Uuid::new_v4().to_string(),
                    "--owner-stdin",
                    "--initial-connection-timeout-ms",
                    "300000",
                ]);
            }
            command
        };
        let candidate = mode == "on-demand";
        let refused = command(env!("CARGO_BIN_EXE_maka"), candidate)
            .output()
            .unwrap();
        assert!(
            !refused.status.success(),
            "uninstalled executable bypassed the managed root"
        );
        let wrong_mode = command(executable, !candidate).output().unwrap();
        assert!(
            !wrong_mode.status.success(),
            "launch mode bypassed deployment policy"
        );
        assert!(
            !fixture.root.join("runtime-rust.sqlite").exists(),
            "admission must precede migrations"
        );

        fixture.child = Some(
            command(executable, candidate)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let registration = fixture.wait_for_registration();
        assert_eq!(
            registration["generation"],
            format!("{}:1", installed["deploymentId"].as_str().unwrap())
        );
        let endpoint = registration["websocketEndpoints"][0].as_str().unwrap();
        let address: SocketAddr = endpoint
            .strip_prefix("ws://")
            .unwrap()
            .strip_suffix("/runtime-host")
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(address.port(), 0);
        let mut health = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
        health
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        health
            .write_all(b"GET /readyz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        health.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");

        let repeated = Command::new(executable)
            .args(["host", "install", "--root"])
            .arg(&fixture.root)
            .args(["--mode", mode])
            .output()
            .unwrap();
        assert!(
            repeated.status.success(),
            "{}",
            String::from_utf8_lossy(&repeated.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&repeated.stdout).unwrap(),
            installed
        );
        let mut activate = Command::new(executable);
        activate.args([
            "host",
            "activate",
            "--framed",
            "--root-id",
            &fixture.root_id,
        ]);
        let connected = activate.output().unwrap();
        assert!(connected.status.success(), "{connected:?}");
        let connected = decode_activation(&connected.stdout);
        assert_eq!(connected["hostEpoch"], registration["hostEpoch"]);
        assert_eq!(connected["pid"], registration["pid"]);
        assert_eq!(connected["endpoint"]["port"], address.port());
        assert!(RootOwner::open(&fixture.root, &namespaces).is_err());
        let retired = Command::new(env!("CARGO_BIN_EXE_maka"))
            .args(["host", "retire", "--root"])
            .arg(&fixture.root)
            .output()
            .unwrap();
        assert!(
            retired.status.success(),
            "{}",
            String::from_utf8_lossy(&retired.stderr)
        );
        assert!(fixture.wait_for_exit().success());
        drop(RootOwner::open(&fixture.root, &namespaces).unwrap());

        if !candidate {
            // Registering an account OS service is an explicit platform
            // acceptance check, never a side effect of ordinary cargo tests.
            continue;
        }
        let started = Instant::now();
        let activated = activate.output().unwrap();
        #[cfg(windows)]
        if requires_containment() {
            // Cargo's own Job forbids independent children. Refusal must not
            // report Ready for a Host that will disappear with the launcher.
            assert!(!activated.status.success(), "{activated:?}");
            let error = decode_activation(&activated.stdout);
            assert_eq!(error["kind"], "error");
            assert!(
                error["error"]["message"]
                    .as_str()
                    .unwrap()
                    .ends_with("(os error 5)")
            );
            drop(RootOwner::open(&fixture.root, &namespaces).unwrap());
            continue;
        }
        assert!(activated.status.success(), "{activated:?}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "inherited capture pipes kept activation alive"
        );
        let frame = decode_activation(&activated.stdout);
        assert_ne!(frame["hostEpoch"], registration["hostEpoch"]);
        assert_eq!(frame["deploymentId"], installed["deploymentId"]);
        // The operator has exited; a relay still has time to establish its client.
        std::thread::sleep(Duration::from_millis(1100));
        let client = Command::new("node")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs"))
            .arg("--activation-frame")
            .arg(String::from_utf8(activated.stdout).unwrap())
            .arg("--root")
            .arg(&fixture.root)
            .output()
            .unwrap();
        assert!(
            client.status.success(),
            "{}",
            String::from_utf8_lossy(&client.stderr)
        );
        let reused = activate.output().unwrap();
        assert!(reused.status.success(), "{reused:?}");
        assert_eq!(decode_activation(&reused.stdout), frame);
        let control = |action: &str| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_maka"));
            command.args([
                "host",
                action,
                "--root-id",
                &fixture.root_id,
                "--expected-deployment-id",
                installed["deploymentId"].as_str().unwrap(),
                "--expected-revision",
                "1",
            ]);
            command
        };
        let restarted = control("restart").output().unwrap();
        assert!(restarted.status.success(), "{restarted:?}");
        let restarted: Value = serde_json::from_slice(&restarted.stdout).unwrap();
        assert_eq!(restarted["kind"], "ready");
        assert_eq!(restarted["deployment"], installed);
        assert_ne!(restarted["host"]["hostEpoch"], frame["hostEpoch"]);
        let stopped = control("stop").output().unwrap();
        assert!(stopped.status.success(), "{stopped:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&stopped.stdout).unwrap()["kind"],
            "stopped"
        );
        drop(RootOwner::open(&fixture.root, &namespaces).unwrap());
        let uninstalled = control("uninstall").output().unwrap();
        assert!(uninstalled.status.success(), "{uninstalled:?}");
        let uninstalled: Value = serde_json::from_slice(&uninstalled.stdout).unwrap();
        assert_eq!(uninstalled["kind"], "unregistered");
        assert_eq!(uninstalled["deployment"]["admission"], "revoked");
        assert_eq!(uninstalled["deployment"]["configRevision"], 2);
        assert_eq!(uninstalled["cleanup"]["kind"], "complete");
        let repeated = control("uninstall").output().unwrap();
        assert!(repeated.status.success(), "{repeated:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&repeated.stdout).unwrap(),
            uninstalled
        );
        assert!(!activate.output().unwrap().status.success());
        let revoked = command(executable, true).output().unwrap();
        assert!(!revoked.status.success());
        assert!(String::from_utf8_lossy(&revoked.stderr).contains("uninstalled"));
        assert!(fixture.root.join("runtime-rust.sqlite").is_file());
        let reinstalled = install.output().unwrap();
        assert!(reinstalled.status.success(), "{reinstalled:?}");
        let reinstalled: Value = serde_json::from_slice(&reinstalled.stdout).unwrap();
        assert_ne!(reinstalled["deploymentId"], installed["deploymentId"]);
        assert_eq!(reinstalled["configRevision"], 1);
        assert_eq!(reinstalled["rootId"], fixture.root_id);
        let ready = activate.output().unwrap();
        assert!(ready.status.success(), "{ready:?}");
        let ready = decode_activation(&ready.stdout);
        assert_eq!(ready["deploymentId"], reinstalled["deploymentId"]);
        assert!(!control("stop").output().unwrap().status.success());
        assert_eq!(decode_activation(&activate.output().unwrap().stdout), ready);
        fixture.retire_registered();
    }
}

fn decode_activation(bytes: &[u8]) -> Value {
    let line = std::str::from_utf8(bytes).unwrap();
    let encoded = line
        .trim()
        .strip_prefix("MAKA_RUNTIME_HOST_ACTIVATION_V1 ")
        .unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap()
}

#[cfg(windows)]
fn requires_containment() -> bool {
    use windows_sys::Win32::System::{
        JobObjects::{
            IsProcessInJob, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            QueryInformationJobObject,
        },
        Threading::GetCurrentProcess,
    };
    let mut member = 0;
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    // SAFETY: queries only this test's enclosing Job; never changes its policy.
    unsafe {
        assert_ne!(
            IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut member),
            0
        );
        if member == 0 {
            return false;
        }
        assert_ne!(
            QueryInformationJobObject(
                std::ptr::null_mut(),
                JobObjectExtendedLimitInformation,
                (&raw mut limits).cast(),
                std::mem::size_of_val(&limits) as u32,
                std::ptr::null_mut(),
            ),
            0
        );
    }
    limits.BasicLimitInformation.LimitFlags
        & (JOB_OBJECT_LIMIT_BREAKAWAY_OK | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK)
        == 0
}
