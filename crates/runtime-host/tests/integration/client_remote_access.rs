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

#![cfg(unix)]

use maka_event_log::{
    EventLog,
    root::{ROOT_DATABASE, RootNamespaces, RootOwner},
};
use maka_runtime_host::server::{Host, local::LocalListener, websocket::WebSocketListener};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_authenticated_remote_authority_and_revocation() {
    let directory = tempfile::Builder::new()
        .prefix("maka-remote-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let namespaces = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let root = directory.path().join("root");
    let owner = RootOwner::create(&root, &namespaces).unwrap();
    let root_id = owner.root_id().to_owned();
    let control = owner.control_directory().to_owned();
    let host = Host::open(owner).await.unwrap();
    let socket = directory.path().join("h.sock");
    let listener = LocalListener::bind(&socket).unwrap();
    let websocket = WebSocketListener::bind(
        "127.0.0.1:0".parse().unwrap(),
        vec!["https://allowed.invalid".into()],
    )
    .await
    .unwrap();
    let address = websocket.local_addr().unwrap();
    let cancellation = CancellationToken::new();
    let server = tokio::spawn(listener.serve_with_websocket(websocket, host, cancellation.clone()));
    let client_socket = socket.clone();
    let client_control = control.clone();
    let client = tokio::task::spawn_blocking(move || {
        Command::new("node")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs"))
            .arg("--socket")
            .arg(client_socket)
            .args(["--root-id", &root_id])
            .args([
                "--remote-access-url",
                &format!("ws://{address}/runtime-host"),
            ])
            .arg("--remote-access-control-directory")
            .arg(client_control)
            .output()
            .unwrap()
    });
    let output = tokio::time::timeout(Duration::from_secs(30), client).await;
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let output = output.unwrap().unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("original-client-remote-access"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("maka_rh_"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("maka_rh_"));
    assert!(!socket.exists(), "shared shutdown removes local socket");
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
    assert!(std::fs::read_dir(control).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("runtime-host-access-delivery-")
    }));
    let log = EventLog::open(&root.join(ROOT_DATABASE)).await.unwrap();
    assert!(
        log.prefix(1000, 1024 * 1024)
            .await
            .unwrap()
            .events
            .is_empty(),
        "remote authority checks and denied session creation must not create execution facts"
    );
    log.close().await.unwrap();
}
