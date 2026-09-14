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
use maka_runtime::artifact::{
    Artifact, ArtifactKind, ArtifactSource, content_digest, upload_artifact_id,
};
use maka_runtime_host::server::{Host, local::LocalListener};
use sqlx::Connection;
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_uploads_queries_retries_disconnects_and_reopens_canonical_blobs() {
    let directory = tempfile::Builder::new()
        .prefix("maka-artifact-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let ns = RootNamespaces {
        ownership: directory.path().join("owners"),
        control: directory.path().join("control"),
    };
    let root = directory.path().join("root");
    let owner = RootOwner::create(&root, &ns).unwrap();
    let root_id = owner.root_id().to_owned();
    drop(owner);
    let mut original = None;
    for reopened in [false, true] {
        let host = Host::open(RootOwner::open(&root, &ns).unwrap())
            .await
            .unwrap();
        let socket = directory.path().join("host.sock");
        let listener = LocalListener::bind(&socket).unwrap();
        let cancellation = CancellationToken::new();
        let server = tokio::spawn(listener.serve(host, cancellation.clone()));
        let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs");
        let workspace = directory.path().to_owned();
        let expected_id = root_id.clone();
        let client = tokio::task::spawn_blocking(move || {
            let mut command = Command::new("node");
            command
                .arg(probe)
                .arg("--socket")
                .arg(socket)
                .args(["--root-id", &expected_id])
                .arg("--artifact-workspace")
                .arg(workspace);
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
        let marker = if reopened {
            "original-client-artifact-reopen"
        } else {
            "original-client-artifact-workflow"
        };
        assert!(String::from_utf8_lossy(&output.stdout).contains(marker));
        let path = root.join(ROOT_DATABASE);
        let log = EventLog::open(&path).await.unwrap();
        let id = upload_artifact_id("session", "durable-upload");
        let record = log
            .get_artifact("session", &id)
            .await
            .unwrap()
            .record
            .unwrap();
        let bytes = "Maka 😀\n".repeat(9000).into_bytes();
        assert_eq!(
            record.summary.as_deref(),
            Some(content_digest(&bytes).as_str())
        );
        assert_eq!(record.source, ArtifactSource::UserUpload);
        assert_eq!(record.turn_id, "durable-upload");
        assert_eq!(record.size_bytes, bytes.len() as u64);
        assert_eq!(
            log.read_artifact_chunk("session", &id, 0, bytes.len())
                .await
                .unwrap()
                .unwrap()
                .bytes,
            bytes
        );
        if let Some(original) = &original {
            assert_eq!(&record, original);
        } else {
            original = Some(record);
        }
        for absent in [
            "uncommitted",
            "disconnect",
            "digest-mismatch",
            "escaped-preview",
        ] {
            assert!(
                log.get_artifact("session", &upload_artifact_id("session", absent))
                    .await
                    .unwrap()
                    .record
                    .is_none()
            );
        }
        assert_eq!(log.prefix(1, 1024).await.unwrap().high_water, 0);
        if !reopened {
            log.commit_artifact(
                Artifact {
                    id: "protected-evidence".into(),
                    session_id: "session".into(),
                    turn_id: "fixture".into(),
                    created_at: 1,
                    name: "proof.txt".into(),
                    kind: ArtifactKind::File,
                    size_bytes: 5,
                    mime_type: Some("text/plain".into()),
                    source: ArtifactSource::ToolResultArchive,
                    summary: None,
                },
                b"proof".to_vec(),
            )
            .await
            .unwrap();
        }
        let page = log.list_artifacts("session", 0, 128).await.unwrap();
        assert_eq!(page.total, 132);
        log.close().await.unwrap();
        let mut observer = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
        )
        .await
        .unwrap();
        let payload: Vec<u8> =
            sqlx::query_scalar("SELECT payload FROM artifacts WHERE session_id = ? AND id = ?")
                .bind("session")
                .bind(&id)
                .fetch_one(&mut observer)
                .await
                .unwrap();
        assert_eq!(payload, bytes);
        sqlx::Connection::close(observer).await.unwrap();
    }
}
