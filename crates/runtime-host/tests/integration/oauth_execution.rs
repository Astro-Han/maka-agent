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

use maka_config::{
    ConfigurationStore,
    network::NetworkConfiguration,
    oauth::enrollment::{LoginCompletion, LoginPreparation},
};
use maka_event_log::{
    EventLog,
    root::{RootNamespaces, RootOwner},
};
use maka_runtime::{
    configuration::*,
    event::{Fact, InvocationOutcome},
    oauth::{LoginStart, Provider, Target},
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

mod live;

/// Seed only a disposable Host-owned credential ticket. This verifies execution
/// after enrollment, not a successful browser/device authorization.
async fn run(secret: String, network: NetworkConfiguration) {
    #[cfg(unix)]
    let temp = {
        use std::os::unix::fs::PermissionsExt;
        tempfile::Builder::new()
            .prefix("maka-oauth-exec-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap()
    };
    #[cfg(windows)]
    let temp = tempfile::tempdir().unwrap();
    let ns = RootNamespaces {
        ownership: temp.path().join("owners"),
        control: temp.path().join("control"),
    };
    let root = temp.path().join("root");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let owner = RootOwner::create(&root, &ns).unwrap();
    let store = Arc::new(ConfigurationStore::for_root(Arc::new(owner)).await.unwrap());
    {
        use policy::network_update::*;
        let before = store.runtime_policy().await.unwrap();
        assert!(matches!(
            store
                .update_network_proxy(
                    Update {
                        expected_policy_revision: before.revision,
                        expected_credential: None,
                        credential: match network.password {
                            Some(secret) if network.proxy.enabled && network.proxy.auth_enabled =>
                                CredentialUpdate::Replace {
                                    secret,
                                    expected_target: None
                                },
                            _ => CredentialUpdate::Delete {},
                        },
                        network_proxy: network.proxy,
                    },
                    1
                )
                .await
                .unwrap(),
            UpdateResult::Committed { .. }
        ));
    }
    let LoginPreparation::Ready(ticket) = store
        .prepare_oauth_login(LoginStart {
            attempt_id: "execution-enrollment".into(),
            target: Target::Create {
                provider_type: Provider::OpenaiCodex,
                slug: None,
                name: None,
            },
        })
        .await
        .unwrap()
    else {
        panic!("new enrollment");
    };
    let identity = ticket.identity().clone();
    assert!(matches!(
        ticket.complete(secret.clone(), 1).await.unwrap(),
        LoginCompletion::Committed(_)
    ));
    let locator = CredentialLocator::Connection {
        connection_id: identity.connection_id,
        kind: ConnectionCredentialKind::OauthToken,
    };
    let credential_before = store.credential_status(locator.clone()).await.unwrap();
    store.shutdown().await.unwrap();
    drop(store);
    let token: Value = serde_json::from_str(&secret).unwrap();
    let access = token["access_token"].as_str().unwrap();
    let mut previous = None;
    for reopened in [false, true] {
        let host = Host::open(RootOwner::open(&root, &ns).unwrap())
            .await
            .unwrap();
        #[cfg(unix)]
        let endpoint = temp.path().join("host.sock");
        #[cfg(windows)]
        let endpoint = std::path::PathBuf::from(format!(
            r"\\.\pipe\maka-oauth-exec-{}",
            uuid::Uuid::new_v4()
        ));
        let listener = LocalListener::bind(&endpoint).unwrap();
        let drain = CancellationToken::new();
        let root_id = host.root_id().to_owned();
        let serving = tokio::spawn(listener.serve(host, drain.clone()));
        let mut command = tokio::process::Command::new("node");
        command
            .kill_on_drop(true)
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/client.mjs"))
            .arg("--socket")
            .arg(endpoint)
            .args(["--root-id", &root_id, "--oauth-execution-workspace"])
            .arg(&workspace);
        if reopened {
            command.arg("--reopened");
        }
        let output = tokio::time::timeout(Duration::from_secs(160), command.output()).await;
        drain.cancel();
        tokio::time::timeout(Duration::from_secs(20), serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let output = output.unwrap().unwrap();
        let log = EventLog::open(&root.join(maka_event_log::root::ROOT_DATABASE))
            .await
            .unwrap();
        let prefix = log.prefix(10000, 8 * 1024 * 1024).await.unwrap();
        let bytes = serde_json::to_vec(&prefix).unwrap();
        for data in [&bytes, &output.stdout, &output.stderr] {
            assert!(
                !String::from_utf8_lossy(data).contains(access),
                "credential escaped trusted storage"
            );
        }
        assert!(
            output.status.success(),
            "original client stdout: {}\nstderr: {}\nterminal: {:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            prefix
                .events
                .iter()
                .filter_map(|item| match &item.event.fact {
                    Fact::InvocationEnded { outcome } => Some(outcome),
                    _ => None,
                })
                .collect::<Vec<_>>()
        );
        let requests = prefix
            .events
            .iter()
            .filter(|item| matches!(item.event.fact, Fact::ModelRequested { .. }))
            .count();
        assert_eq!(
            requests,
            if reopened { 2 } else { 1 },
            "reopen must not replay the first request"
        );
        assert!(
            prefix
                .events
                .iter()
                .filter_map(|item| match &item.event.fact {
                    Fact::InvocationEnded { outcome } => Some(outcome),
                    _ => None,
                })
                .all(|outcome| *outcome == InvocationOutcome::Completed)
        );
        if let Some(previous) = previous {
            let before: Vec<Value> = serde_json::from_value(previous).unwrap();
            let after = serde_json::to_value(&prefix.events).unwrap();
            assert_eq!(&after.as_array().unwrap()[..before.len()], &before);
        }
        previous = Some(serde_json::to_value(&prefix.events).unwrap());
        log.close().await.unwrap();
    }
    let store = ConfigurationStore::for_root(Arc::new(RootOwner::open(&root, &ns).unwrap()))
        .await
        .unwrap();
    assert_eq!(
        store.credential_status(locator.clone()).await.unwrap(),
        credential_before,
        "read-only live acceptance must not rotate credentials"
    );
    assert!(
        store
            .credential_secret(&locator, None)
            .await
            .unwrap()
            .is_some_and(|stored| stored == secret)
    );
    store.close().await.unwrap();
    temp.close().unwrap();
}
