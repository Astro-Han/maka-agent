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

use super::{Host, HostError, local::LocalListener, websocket::WebSocketListener};
use maka_transport::ndjson;
use std::sync::Arc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// One admission/drain owner for every transport belonging to this Host.
pub(super) async fn serve(
    mut local: LocalListener,
    websocket: Option<WebSocketListener>,
    host: Arc<Host>,
    cancellation: CancellationToken,
) -> Result<(), HostError> {
    let mut connections = JoinSet::new();
    // Untrusted remote sockets must not consume local recovery/revocation
    // capacity, including while they have not authenticated yet.
    let remote_capacity = Arc::new(tokio::sync::Semaphore::new(64));
    let cancel = CancellationToken::new();
    let _cancel_on_exit = cancel.clone().drop_guard();
    let mut next_expiry = host.configuration.next_access_credential_expiry().await?;
    let result = loop {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => break Ok(()),
            _ = host.draining.cancelled() => break Ok(()),
            _ = host.access_changed.notified() => {
                next_expiry = match host.configuration.next_access_credential_expiry().await {
                    Ok(expiry) => expiry,
                    Err(error) => break Err(error.into()),
                };
            },
            result = async {
                match next_expiry {
                    Some(at) => {
                        let now = super::configuration::now()?;
                        tokio::time::sleep(std::time::Duration::from_millis(at.saturating_sub(now))).await;
                        Ok::<(), HostError>(())
                    },
                    None => std::future::pending().await,
                }
            } => {
                if let Err(error) = result { break Err(error); }
                if let Err(error) = super::access::expire(&host).await { break Err(error); }
                next_expiry = match host.configuration.next_access_credential_expiry().await {
                    Ok(expiry) => expiry,
                    Err(error) => break Err(error.into()),
                };
            },
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                if let Err(error) = result { host.record_diagnostic(format_args!("host connection task failed: {error}")); }
            },
            accepted = local.accept(), if connections.len() < 128 => {
                let socket = match accepted {
                    Ok(socket) => socket,
                    Err(error) => break Err(error.into()),
                };
                let host = host.clone();
                let token = cancel.child_token();
                connections.spawn(async move {
                    let (reader, writer) = ndjson::split(socket, token.clone());
                    let _cancel_on_exit = token.clone().drop_guard();
                    report(&host, host.clone().authorized_connection(
                        reader,
                        writer,
                        super::authority::Authority::LocalOwner,
                        token,
                    ).await);
                });
            },
            accepted = async { websocket.as_ref().unwrap().listener.accept().await },
                if websocket.is_some() && connections.len() < 128 && remote_capacity.available_permits() > 0 => {
                let (socket, _) = match accepted {
                    Ok(socket) => socket,
                    Err(error) => break Err(error.into()),
                };
                let host = host.clone();
                let token = cancel.child_token();
                let origins = websocket.as_ref().unwrap().allowed_origins.clone();
                let permit = remote_capacity.clone().try_acquire_owned().expect("single accept owner checked capacity");
                connections.spawn(async move {
                    let _permit = permit;
                    let _cancel_on_exit = token.clone().drop_guard();
                    report(&host, super::websocket::connection(socket, host.clone(), origins, token).await);
                });
            }
        }
    };
    host.draining.cancel();
    host.plugin_remotes.close();
    host.record_diagnostic("Host admission closed; awaiting accepted response flush");
    host.requests.close();
    host.requests.wait().await;
    // Stop orchestration before Host-owned executions and storage. Independent
    // accepted executions retain their ordinary Host shutdown semantics.
    host.plugin_tasks.close();
    host.plugin_tasks.wait().await;
    // Registry drain closes provider transports. Admitted ordinary responses
    // must finish flushing before those shared cancellation tokens are closed.
    host.capabilities.begin_drain();
    cancel.cancel();
    while connections.join_next().await.is_some() {}
    host.executions.shutdown().await;
    host.shells.shutdown().await;
    host.capabilities.shutdown().await;
    let (log_closed, configuration_closed) =
        tokio::join!(host.log.shutdown(), host.configuration.shutdown());
    host.record_diagnostic("Host execution, transport and storage drain finished");
    result
        .and(log_closed.map_err(Into::into))
        .and(configuration_closed.map_err(Into::into))
}

fn report(host: &Host, result: Result<(), HostError>) {
    if let Err(error) = result {
        // Individual malformed/closed connections never terminate the Host.
        host.record_diagnostic(format_args!("host connection closed: {error}"));
    }
}
