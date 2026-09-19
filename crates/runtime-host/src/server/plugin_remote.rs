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

mod registry;
mod worker;
pub(super) use registry::Registry;

use super::Host;
use futures_util::FutureExt;
use maka_plugins::remote::{Caller, Error, Handler, validate_payload};
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    plugin::{RemoteKind, RemoteRequest, RemoteResult},
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

pub(super) async fn execute(
    host: &Arc<Host>,
    connection: Uuid,
    client: &str,
    authority: &super::authority::Authority,
    request: RemoteRequest,
) -> Result<RemoteResult, OperationError> {
    match request {
        RemoteRequest::OpenDocument => Ok(RemoteResult::Document {
            document: host.plugin_remotes.open(connection)?,
        }),
        RemoteRequest::CloseDocument { document } => {
            if let Some(owner) = host.plugin_remotes.close_document(connection, document)? {
                owner.drained().await?;
                host.plugin_remotes.forget_document(connection, document);
            }
            Ok(RemoteResult::Closed)
        }
        RemoteRequest::Next { document, stream } => {
            let stream = host
                .plugin_remotes
                .get(connection, document)?
                .stream(stream)?;
            Ok(match stream.next().await.map_err(failure)? {
                worker::Item::Value(item) => RemoteResult::Item { item },
                worker::Item::End => RemoteResult::End,
                worker::Item::Pending => RemoteResult::Pending,
            })
        }
        RemoteRequest::Close { document, stream } => {
            let document = host.plugin_remotes.get(connection, document)?;
            if let Ok(stream) = document.stream(stream) {
                stream.close().await.map_err(failure)?;
            }
            document.check_cleanup()?;
            Ok(RemoteResult::Closed)
        }
        request => {
            let (binding, target, call) = match request {
                RemoteRequest::Bind { binding } => (binding, None, None),
                RemoteRequest::Call {
                    binding,
                    target,
                    document,
                    input,
                } => (
                    binding,
                    Some(target),
                    Some((document, input, RemoteKind::Method)),
                ),
                RemoteRequest::Open {
                    binding,
                    target,
                    document,
                    input,
                } => (
                    binding,
                    Some(target),
                    Some((document, input, RemoteKind::Stream)),
                ),
                _ => unreachable!(),
            };
            // Session identity is canonical Host-local, not a Desktop projection.
            let gate = host.executions.lock_admission().await;
            if let Some(session) = &binding.session_id {
                host.log
                    .get_session::<crate::session::SessionConfiguration>(session)
                    .await
                    .map_err(|error| failure(Error::Provider(error.to_string())))?
                    .ok_or_else(|| {
                        failure(Error::Invalid("Remote Session does not exist".into()))
                    })?;
            }
            let bound = host.plugins.bind_remote(&binding, target.as_ref())?;
            if bound.endpoint.value.access == maka_plugins::remote::Access::HostPaths
                && !authority.can_use_host_paths()
            {
                return Err(OperationError {
                    code: Code::Unauthorized,
                    message: "Remote endpoint requires Host path access".into(),
                });
            }
            let Some((document, input, stream)) = call else {
                return Ok(RemoteResult::Bound {
                    target: bound.target,
                    handler: match bound.endpoint.value.handler {
                        Handler::Method(_) => RemoteKind::Method,
                        Handler::Stream(_) => RemoteKind::Stream,
                    },
                });
            };
            let reservation = host.plugin_remotes.get(connection, document)?.reserve()?;
            let caller = Caller {
                connection_id: connection,
                client_instance_id: client.into(),
                document_id: document,
                session_id: binding.session_id,
                cancellation: reservation.document.cancellation.child_token(),
            };
            match (&bound.endpoint.value.handler, stream) {
                (Handler::Stream(provider), RemoteKind::Stream) => {
                    let provider = provider.clone();
                    let (stream, opened) = worker::start(
                        reservation,
                        bound,
                        provider,
                        input,
                        caller,
                        &host.plugin_tasks,
                    )?;
                    drop(gate);
                    opened
                        .await
                        .map_err(|_| failure(Error::CleanupUnconfirmed))?
                        .map_err(failure)?;
                    Ok(RemoteResult::Opened { stream })
                }
                (Handler::Method(method), RemoteKind::Method) => {
                    let method = method.clone();
                    let leases = bound.admit()?;
                    let (send, receive) = tokio::sync::oneshot::channel();
                    host.plugin_tasks.spawn(async move {
                        let _leases = leases;
                        let result = std::panic::AssertUnwindSafe(call_method(
                            &bound, method, input, caller,
                        ))
                        .catch_unwind()
                        .await
                        .unwrap_or(Err(Error::CleanupUnconfirmed));
                        if matches!(result, Err(Error::CleanupUnconfirmed)) {
                            reservation.document.cleanup_failed();
                            bound
                                .endpoint
                                .owner
                                .cleanup_failed("Remote method cleanup is unconfirmed".into());
                        }
                        let _ = send.send(result);
                        drop(reservation);
                    });
                    drop(gate);
                    let value = receive
                        .await
                        .map_err(|_| failure(Error::CleanupUnconfirmed))?
                        .map_err(failure)?;
                    Ok(RemoteResult::Value { value })
                }
                _ => Err(failure(Error::Invalid(
                    "Remote handler kind does not match request".into(),
                ))),
            }
        }
    }
}

async fn call_method(
    bound: &crate::plugins::remote::Bound,
    method: Arc<dyn maka_plugins::remote::Method>,
    input: Value,
    caller: Caller,
) -> Result<Value, Error> {
    let cancellation = caller.cancellation.clone();
    let _cancel_on_exit = cancellation.clone().drop_guard();
    let mut call = method.call(input, caller);
    let result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(Error::Cancelled),
        _ = bound.client.retired() => Err(Error::Retired),
        _ = bound.endpoint.retired() => Err(Error::Retired),
        _ = tokio::time::sleep(Duration::from_secs(30)) => Err(Error::Cancelled),
        result = &mut call => {
            if matches!(result, Err(Error::CleanupUnconfirmed)) {
                bound.endpoint.owner.cleanup_failed("Remote method cleanup is unconfirmed".into());
            }
            return result.and_then(|value| { validate_payload(&value)?; Ok(value) });
        }
    };
    cancellation.cancel();
    match tokio::time::timeout(Duration::from_secs(5), call).await {
        Ok(result) if !matches!(result, Err(Error::CleanupUnconfirmed)) => {}
        _ => {
            bound
                .endpoint
                .owner
                .cleanup_failed("Remote method ignored cancellation".into());
            return Err(Error::CleanupUnconfirmed);
        }
    };
    result
}
fn failure(error: Error) -> OperationError {
    OperationError {
        code: match error {
            Error::Invalid(_) => Code::InvalidRequest,
            Error::Retired | Error::Cancelled => Code::OperationConflict,
            Error::Provider(_) | Error::CleanupUnconfirmed => Code::OperationUnavailable,
        },
        message: error.to_string(),
    }
}
