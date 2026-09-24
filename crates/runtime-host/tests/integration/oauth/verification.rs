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

use super::support::*;
use maka_config::{
    ConfigurationStore,
    oauth::enrollment::{LoginCompletion, LoginPreparation},
};
use maka_runtime::{
    configuration::*,
    oauth::{LoginStart, Target},
    provider::{AuthenticationInput, Credential},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// The public provider chooses its endpoint; Host transport applies the pinned
/// proxy without leaking origin credentials to the CONNECT hop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_verification_uses_pinned_proxy_and_persists_observations() {
    let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Fixture::new(Some(proxy.local_addr().unwrap().port())).await;
    let store = Arc::new(
        ConfigurationStore::for_root(Arc::new(fixture.owner()))
            .await
            .unwrap(),
    );
    let mut ids = Vec::new();
    for (name, provider, endpoint, secret) in [
        (
            "subscription",
            codex(),
            "https://chatgpt.com/backend-api/codex",
            json!({"access_token":"fixture-access", "refresh_token":"must-not-refresh",
                "expires_at":9_007_199_254_740_991_u64})
            .to_string(),
        ),
        (
            "api",
            json!({"packageId":"maka.providers", "entryId":"maka.providers", "scope":"profile", "name":"openai"}),
            "https://provider.invalid/v1",
            "fixture-access".into(),
        ),
    ] {
        let LoginPreparation::Ready(ticket) = store
            .prepare_oauth_login(LoginStart {
                attempt_id: name.into(),
                target: Target::Create {
                    provider: serde_json::from_value(provider).unwrap(),
                    configuration: json!({"baseUrl":endpoint}),
                    slug: name.into(),
                    name: name.into(),
                },
                authentication: AuthenticationInput {
                    method: "fixture".into(),
                    input: json!({}),
                },
            })
            .await
            .unwrap()
        else {
            panic!("enrollment")
        };
        let id = ticket.identity().connection_id.clone();
        assert!(ticket.claim().await.unwrap());
        assert!(matches!(
            ticket
                .complete(
                    Credential {
                        secret,
                        refresh_at: None
                    },
                    1
                )
                .await
                .unwrap(),
            LoginCompletion::Committed(_)
        ));
        let row = store
            .catalog()
            .await
            .unwrap()
            .connections
            .into_iter()
            .find(|row| row.connection_id == id)
            .unwrap();
        assert!(matches!(
            store
                .update_connection(UpdateCatalogConnectionInput {
                    expected: ConnectionVersionBasis {
                        connection_id: id.clone(),
                        revision: row.revision
                    },
                    changes: ConnectionCatalogEntryUpdate {
                        name: row.name,
                        configuration: row.configuration,
                        enabled: true,
                        enabled_model_ids: vec!["fixture-model".into()],
                        model_overrides: Patch::Keep,
                        request_body_overlay: Patch::Keep,
                    },
                })
                .await
                .unwrap(),
            CatalogMutationResult::Committed { .. }
        ));
        ids.push(id);
    }
    store.shutdown().await.unwrap();
    drop(store);
    let (host, drain, server) = fixture.serve().await;
    let mut peer = Peer::new(host.clone(), "oauth-verification").await;
    crate::javascript_plugins::ready(&mut peer).await;
    for (id, host) in [(&ids[0], "chatgpt.com"), (&ids[1], "provider.invalid")] {
        let observed = async {
            let (mut socket, _) = proxy.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(socket.read_u8().await.unwrap());
                assert!(bytes.len() < 8192);
            }
            let headers = String::from_utf8(bytes).unwrap();
            assert!(headers.starts_with(&format!("CONNECT {host}:443 HTTP/1.1\r\n")));
            assert!(
                !headers.contains("fixture-access"),
                "origin credential leaked to proxy"
            );
            socket
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        };
        let probe = peer.rpc(
            "connection.test.run",
            json!({"connectionId":id, "modelId":"fixture-model"}),
        );
        let (reply, observed) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                probe,
                tokio::time::timeout(Duration::from_secs(5), observed)
            )
        })
        .await
        .unwrap();
        assert!(
            observed.is_ok(),
            "provider did not use the configured proxy: {reply}"
        );
        assert_eq!(reply["result"]["kind"], "committed", "{reply}");
        assert_eq!(reply["result"]["test"]["kind"], "failed", "{reply}");
        assert_eq!(reply["result"]["test"]["errorClass"], "network", "{reply}");
        assert_eq!(reply["result"]["test"]["statusCode"], Value::Null);
    }
    peer.close().await;
    drain.cancel();
    finish(server).await;
    drop(host);
    let store = ConfigurationStore::for_root(Arc::new(fixture.owner()))
        .await
        .unwrap();
    for id in &ids {
        let row = store
            .catalog()
            .await
            .unwrap()
            .connections
            .into_iter()
            .find(|row| row.connection_id == *id)
            .unwrap();
        assert_eq!(row.last_test.unwrap().status, ConnectionTestStatus::Error);
    }
    store.close().await.unwrap();
    fixture.temp.close().unwrap();
}
