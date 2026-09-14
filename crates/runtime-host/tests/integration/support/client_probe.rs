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

use maka_event_log::{
    EventLog,
    root::{ROOT_DATABASE, RootNamespaces, RootOwner},
};
use maka_runtime_host::server::{Host, local::LocalListener};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

/// Only lifecycle is shared; each scenario asserts its own canonical evidence.
pub(crate) struct ClientFixture {
    _directory: tempfile::TempDir,
    namespaces: RootNamespaces,
    root: PathBuf,
    root_id: String,
    global_instructions: PathBuf,
    pub(crate) workspace: PathBuf,
}

impl ClientFixture {
    pub(crate) fn owner(&self) -> RootOwner {
        RootOwner::open(&self.root, &self.namespaces).unwrap()
    }

    pub(crate) fn new(prefix: &str) -> Self {
        #[cfg(unix)]
        let directory = tempfile::Builder::new()
            .prefix(prefix)
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        #[cfg(windows)]
        let directory = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let namespaces = RootNamespaces {
            ownership: directory.path().join("owners"),
            control: directory.path().join("control"),
        };
        let root = directory.path().join("root");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let global_instructions = directory.path().join(".maka");
        std::fs::create_dir(&global_instructions).unwrap();
        let owner = RootOwner::create(&root, &namespaces).unwrap();
        let root_id = owner.root_id().to_owned();
        drop(owner);
        Self {
            _directory: directory,
            namespaces,
            root,
            root_id,
            global_instructions,
            workspace,
        }
    }

    pub(crate) async fn run(&self, flag: &str, reopened: bool, marker: &str) {
        self.run_with_options(flag, reopened, marker, Default::default())
            .await;
    }

    pub(crate) async fn run_with_options(
        &self,
        flag: &str,
        reopened: bool,
        marker: &str,
        options: maka_runtime_host::server::HostOptions,
    ) {
        let host = Host::open_with_options(
            RootOwner::open(&self.root, &self.namespaces).unwrap(),
            Some(self.global_instructions.clone()),
            options,
        )
        .await
        .unwrap();
        #[cfg(unix)]
        let socket = self._directory.path().join("h.sock");
        #[cfg(windows)]
        let socket = PathBuf::from(format!(r"\\.\pipe\maka-test-{}", uuid::Uuid::new_v4()));
        let cancellation = CancellationToken::new();
        let mut server = tokio::spawn(
            LocalListener::bind(&socket)
                .unwrap()
                .serve(host, cancellation.clone()),
        );
        let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs");
        let mut command = tokio::process::Command::new("node");
        command
            .kill_on_drop(true)
            .env("MAKA_TEST_STATE_ROOT", &self.root)
            .env("MAKA_TEST_GLOBAL_INSTRUCTIONS", &self.global_instructions)
            .arg(probe)
            .arg("--socket")
            .arg(socket)
            .args(["--root-id", &self.root_id])
            .arg(flag)
            .arg(&self.workspace);
        if reopened {
            command.arg("--reopened");
        }
        let output = tokio::time::timeout(Duration::from_secs(60), command.output()).await;
        // A failed probe must release the writer before assertions or reopening.
        cancellation.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(10), &mut server).await;
        if stopped.is_err() {
            server.abort();
            let _ = server.await;
        }
        stopped
            .unwrap_or_else(|error| panic!("Host did not drain: {error}; client: {output:?}"))
            .unwrap()
            .unwrap();
        let output = output.unwrap().unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains(marker));
    }

    pub(crate) async fn log(&self) -> EventLog {
        EventLog::open(&self.root.join(ROOT_DATABASE))
            .await
            .unwrap()
    }
}
