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

use maka_config::ConfigurationStore;
use maka_event_log::{
    EventLog,
    root::{ROOT_DATABASE, RootNamespaces, RootOwner},
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::Value;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_access_delivery_revocation_and_authority_survive_reopen() {
    let directory = tempfile::Builder::new()
        .prefix("maka-access-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let ns = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let root = directory.path().join("root");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let owner = RootOwner::create(&root, &ns).unwrap();
    let root_id = owner.root_id().to_owned();
    let control = owner.control_directory().to_owned();
    drop(owner);
    let mut original = None;
    for reopened in [false, true] {
        let host = Host::open(RootOwner::open(&root, &ns).unwrap())
            .await
            .unwrap();
        let socket = directory.path().join("h.sock");
        let listener = LocalListener::bind(&socket).unwrap();
        let cancellation = CancellationToken::new();
        let server = tokio::spawn(listener.serve(host, cancellation.clone()));
        let client_workspace = workspace.clone();
        let client_control = control.clone();
        let expected_id = root_id.clone();
        let client = tokio::task::spawn_blocking(move || {
            let mut command = Command::new("node");
            command
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs"))
                .arg("--socket")
                .arg(socket)
                .args(["--root-id", &expected_id])
                .arg("--access-workspace")
                .arg(client_workspace)
                .arg("--access-control-directory")
                .arg(client_control);
            if reopened {
                command.arg("--reopened");
            }
            command.output().unwrap()
        });
        let output = tokio::time::timeout(Duration::from_secs(30), client)
            .await
            .unwrap()
            .unwrap();
        cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(if reopened {
                "original-client-access-reopened"
            } else {
                "original-client-access"
            })
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("maka_rh_"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("maka_rh_"));
        assert!(
            std::fs::read_dir(&control).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("runtime-host-access-delivery-")
            }),
            "shutdown must remove unconsumed private deliveries"
        );
        let saved: Value =
            serde_json::from_slice(&std::fs::read(workspace.join("access.json")).unwrap()).unwrap();
        let store = ConfigurationStore::for_root(Arc::new(RootOwner::open(&root, &ns).unwrap()))
            .await
            .unwrap();
        let active = store.active_access_credentials().await.unwrap();
        assert_eq!(active.len(), if reopened { 0 } else { 2 });
        for name in ["revoked", "active"] {
            let hash = saved[name]["credentialHash"].as_str().unwrap().to_owned();
            let authenticated = store.authenticate_access_credential(hash, 0).await.unwrap();
            if name == "active" && !reopened {
                let credential = authenticated.unwrap();
                assert_eq!(credential.credential_id, saved[name]["credentialId"]);
                assert_eq!(credential.grants, ["host.status", "session.catalog.query"]);
                assert!(credential.can_publish_client_capabilities);
                assert!(!credential.can_use_host_paths);
            } else {
                assert!(authenticated.is_none());
            }
        }
        store.close().await.unwrap();
        // Read committed SQL records to distinguish durable revocation from deletion.
        let mut sql = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(root.join("configuration-rust.sqlite"))
                .read_only(true),
        )
        .await
        .unwrap();
        let documents: Vec<String> = sqlx::query_scalar("SELECT document FROM access_credentials")
            .fetch_all(&mut sql)
            .await
            .unwrap();
        assert_eq!(documents.len(), 3);
        for document in documents {
            assert!(!document.contains("maka_rh_"));
            let credential: Value = serde_json::from_str(&document).unwrap();
            let name = ["revoked", "active", "pending"]
                .into_iter()
                .find(|name| saved[*name]["credentialId"] == credential["credentialId"])
                .unwrap();
            assert_eq!(
                credential["state"]["kind"] == "revoked",
                reopened || name == "revoked"
            );
            if name != "pending" {
                assert_eq!(credential["credentialHash"], saved[name]["credentialHash"]);
            }
        }
        sql.close().await.unwrap();
        let log = EventLog::open(&root.join(ROOT_DATABASE)).await.unwrap();
        let prefix = log.prefix(1000, 1024 * 1024).await.unwrap();
        assert!(
            prefix.events.is_empty(),
            "access control must not create execution facts"
        );
        let bytes = serde_json::to_vec(&prefix).unwrap();
        if let Some(original) = &original {
            assert_eq!(&bytes, original, "reopen preserves execution facts exactly");
        } else {
            original = Some(bytes);
        }
        log.close().await.unwrap();
    }
}
