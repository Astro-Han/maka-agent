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
    oauth::{
        OAuthCredential,
        enrollment::{LoginCompletion, LoginPreparation},
    },
};
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_model::oauth::Client;
use maka_runtime::{configuration::*, oauth::*};
use serde_json::json;
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        ServerConfig,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    },
};

pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub store: Arc<ConfigurationStore>,
    pub snapshot: OAuthCredential,
}
impl Fixture {
    pub async fn new(provider: Provider) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let owner = RootOwner::create(
            &temp.path().join("root"),
            &RootNamespaces {
                ownership: temp.path().join("owners"),
                control: temp.path().join("control"),
            },
        )
        .unwrap();
        let store = Arc::new(ConfigurationStore::for_root(Arc::new(owner)).await.unwrap());
        let LoginPreparation::Ready(ticket) = store
            .prepare_oauth_login(LoginStart {
                attempt_id: "refresh-fixture".into(),
                target: Target::Create {
                    provider_type: provider,
                    slug: None,
                    name: None,
                },
            })
            .await
            .unwrap()
        else {
            panic!("new login");
        };
        let id = ticket.identity().connection_id.clone();
        assert!(matches!(
            ticket
                .complete(
                    json!({"access_token":"gho_old","refresh_token":"old",
            "expires_at":0})
                    .to_string(),
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
        let snapshot = store
            .oauth_credential(ConnectionCredentialTarget {
                connection_id: id,
                revision: row.revision,
                slug: row.slug,
                provider_type: row.provider_type,
                effective_base_url: validation::normalize_base_url(
                    Some(validation::provider_default_base_url(provider.as_str()).unwrap()),
                    None,
                )
                .unwrap()
                .unwrap(),
            })
            .await
            .unwrap()
            .unwrap();
        Self {
            temp,
            store,
            snapshot,
        }
    }

    pub async fn sql(&self) -> sqlx::SqliteConnection {
        use sqlx::Connection;
        sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .filename(self.temp.path().join("root/configuration-rust.sqlite")),
        )
        .await
        .unwrap()
    }
}

pub struct Grant {
    pub client: Client,
    pub admitted: oneshot::Receiver<()>,
    pub release: oneshot::Sender<()>,
    pub server: tokio::task::JoinHandle<String>,
}
pub async fn grant(access: &str, refresh: &str, seconds: u64) -> Grant {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let policy = maka_network::Policy::from_settings(
        &policy::NetworkProxy {
            enabled: true,
            protocol: policy::ProxyProtocol::Http,
            host: "127.0.0.1".into(),
            port: listener.local_addr().unwrap().port(),
            auth_enabled: false,
            username: String::new(),
            bypass_list: vec![],
            auto_bypass_domains: vec![],
        },
        None,
    )
    .unwrap();
    let client =
        Client::with_http_builder(policy.client_builder().danger_accept_invalid_certs(true))
            .unwrap();
    let payload =
        json!({"access_token":access, "refresh_token":refresh, "expires_in":seconds}).to_string();
    let (admit, admitted) = oneshot::channel();
    let (release, ready) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let connect = head(&mut tcp).await;
        assert!(connect.starts_with("CONNECT "));
        tcp.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .unwrap();
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![
                    CertificateDer::from_pem_slice(include_bytes!(
                        "../../../../network/tests/fixtures/localhost.pem"
                    ))
                    .unwrap(),
                ],
                PrivateKeyDer::from_pem_slice(include_bytes!(
                    "../../../../network/tests/fixtures/localhost.key"
                ))
                .unwrap(),
            )
            .unwrap();
        let mut tls = TlsAcceptor::from(Arc::new(config))
            .accept(tcp)
            .await
            .unwrap();
        let headers = head(&mut tls).await;
        let length: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        tls.read_exact(&mut body).await.unwrap();
        admit.send(()).unwrap();
        ready.await.unwrap();
        tls.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}", payload.len()).as_bytes()).await.unwrap();
        tls.shutdown().await.unwrap();
        String::from_utf8(body).unwrap()
    });
    Grant {
        client,
        admitted,
        release,
        server,
    }
}
async fn head(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        assert!(bytes.len() < 16 * 1024);
        bytes.push(stream.read_u8().await.unwrap());
    }
    String::from_utf8(bytes).unwrap()
}
