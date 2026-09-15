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
use maka_event_log::root::{RootNamespaces, RootOwner};
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    process::{Command, Stdio},
    time::Duration,
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

        let command = |executable: &str, candidate: bool| {
            let mut command = Command::new(executable);
            command
                .args([
                    "host",
                    if candidate { "candidate" } else { "serve" },
                    "--root",
                ])
                .arg(&fixture.root);
            if candidate {
                command.args([
                    "--expected-root-id",
                    &fixture.root_id,
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
    }
}
