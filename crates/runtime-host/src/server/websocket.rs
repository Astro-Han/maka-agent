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

mod upgrade;

use super::{Host, HostError, authority::Authority};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::broadcast,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

/// Plain HTTP is restricted to loopback. TLS/remote network exposure is a
/// separate listener configuration, not implied by accepting remote principals.
pub struct WebSocketListener {
    pub(super) listener: TcpListener,
    pub(super) allowed_origins: Arc<[String]>,
}

impl WebSocketListener {
    pub async fn bind(
        address: SocketAddr,
        allowed_origins: Vec<String>,
    ) -> Result<Self, HostError> {
        if !address.ip().is_loopback() {
            return Err("Plain WebSocket listener requires a loopback address".into());
        }
        if allowed_origins
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != allowed_origins.len()
        {
            return Err("WebSocket Origin allowlist contains duplicates".into());
        }
        Ok(Self {
            listener: TcpListener::bind(address).await?,
            allowed_origins: allowed_origins.into(),
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

pub(super) async fn connection(
    socket: TcpStream,
    host: Arc<Host>,
    allowed_origins: Arc<[String]>,
    cancel: CancellationToken,
) -> Result<(), HostError> {
    // A separate handle can stop network delivery immediately on revocation
    // while the admitted request retains ownership of its durable completion.
    let socket = socket.into_std()?;
    let abort = socket.try_clone()?;
    let socket = TcpStream::from_std(socket)?;
    // Subscribe before the first authentication. Recheck after the HTTP upgrade
    // to close the race between authentication and accepting a protocol hello.
    let mut revocations = host.access_revocations.subscribe();
    let upgraded = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(()),
        result = timeout(Duration::from_secs(5), upgrade::accept(socket, host.clone(), allowed_origins)) => result??,
    };
    let Some((socket, hash)) = upgraded else {
        return Ok(());
    };
    let Some(credential) = host
        .configuration
        .authenticate_access_credential(hash, super::configuration::now()?)
        .await?
    else {
        return Ok(());
    };
    let credential_id = credential.credential_id.clone();
    let (reader, writer) = maka_transport::websocket::split(socket, cancel.clone())?;
    let connection = host.authorized_connection(
        reader,
        writer,
        Authority::Managed(Box::new(credential)),
        cancel.clone(),
    );
    tokio::pin!(connection);
    loop {
        tokio::select! {
            biased;
            revocation = revocations.recv(), if !cancel.is_cancelled() => {
                match revocation {
                    Ok(id) if id != credential_id => continue,
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_))
                        | Err(broadcast::error::RecvError::Closed) => {
                            cancel.cancel();
                            let _ = abort.shutdown(std::net::Shutdown::Both);
                        },
                }
                // Do not drop admitted dispatch: it still owns its durable
                // commit/effect boundary, although this transport is now closed.
            },
            result = &mut connection => return result,
        }
    }
}
