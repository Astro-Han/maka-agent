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
    oauth::{LoginStart, Provider, Target},
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// OAuth endpoints are fixed by the provider. A refused CONNECT exercises Host
/// authentication/routing/proxy and durable failure without bypassing TLS trust.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oauth_verification_uses_readiness_or_pinned_proxy_and_persists_observations() {
    let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Fixture::new(Some(proxy.local_addr().unwrap().port())).await;
    let store = Arc::new(
        ConfigurationStore::for_root(Arc::new(fixture.owner()))
            .await
            .unwrap(),
    );
    let mut ids = Vec::new();
    for provider in [
        Provider::OpenaiCodex,
        Provider::GithubCopilot,
        Provider::XaiOauth,
    ] {
        let LoginPreparation::Ready(ticket) = store
            .prepare_oauth_login(LoginStart {
                attempt_id: provider.as_str().into(),
                target: Target::Create {
                    provider_type: provider,
                    slug: None,
                    name: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("enrollment")
        };
        let id = ticket.identity().connection_id.clone();
        let secret = json!({"access_token":"fixture-access", "refresh_token":"must-not-refresh",
            "expires_at":9_007_199_254_740_991_u64})
        .to_string();
        assert!(matches!(
            ticket.complete(secret, 1).await.unwrap(),
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
                        base_url: row.base_url,
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
    let ready = peer
        .rpc(
            "connection.test.run",
            json!({"connectionId":ids[0], "modelId":"fixture-model"}),
        )
        .await;
    assert_eq!(ready["result"]["kind"], "committed", "{ready}");
    assert_eq!(ready["result"]["test"]["kind"], "verified", "{ready}");
    assert_eq!(ready["result"]["test"]["modelId"], "fixture-model");
    // Readiness must not send a synthetic inference request or require that an
    // enabled custom model appears in a remotely fetched inventory.
    assert!(
        tokio::time::timeout(Duration::from_millis(20), proxy.accept())
            .await
            .is_err()
    );
    for (id, host) in [(&ids[1], "api.githubcopilot.com"), (&ids[2], "api.x.ai")] {
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
        let (reply, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(probe, observed)
        })
        .await
        .unwrap();
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
    for (id, expected) in ids.iter().zip([
        ConnectionTestStatus::Verified,
        ConnectionTestStatus::Error,
        ConnectionTestStatus::Error,
    ]) {
        let row = store
            .catalog()
            .await
            .unwrap()
            .connections
            .into_iter()
            .find(|row| row.connection_id == *id)
            .unwrap();
        assert_eq!(row.last_test.unwrap().status, expected);
    }
    store.close().await.unwrap();
    fixture.temp.close().unwrap();
}
