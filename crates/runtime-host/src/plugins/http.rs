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

mod worker;
use crate::execution::Executions;
use futures_util::future::BoxFuture;
use maka_plugins::{
    call::Scope,
    fiber::Context,
    http::{Body, Client, Error, Head, Method, Request, Response},
};
use maka_runtime::tools::ToolError;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::{
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

pub(super) struct Http {
    host: Weak<Executions>,
    owner: Context,
    permissions: Arc<dyn maka_plugins::permissions::Access>,
    client: tokio::sync::Mutex<Option<(maka_network::Policy, reqwest::Client)>>,
    capacity: Arc<tokio::sync::Semaphore>,
}
impl Http {
    pub fn new(
        host: Weak<Executions>,
        owner: Context,
        permissions: Arc<dyn maka_plugins::permissions::Access>,
    ) -> Self {
        Self {
            host,
            owner,
            permissions,
            client: Default::default(),
            capacity: Arc::new(tokio::sync::Semaphore::new(32)),
        }
    }
}
impl Client for Http {
    fn request(&self, call: Scope, input: Request) -> BoxFuture<'_, Result<Response, Error>> {
        Box::pin(async move {
            let _lease = self.owner.admit().map_err(|_| Error::Denied)?;
            let host = self.host.upgrade().ok_or(Error::Denied)?;
            if !host.plugin_calls.owns(&call) || call.cancellation.is_cancelled() {
                return Err(Error::Denied);
            }
            let permit = self.capacity.clone().try_acquire_owned().map_err(|_| {
                Error::Failed("close unused HTTP responses before starting more".into())
            })?;
            let url = reqwest::Url::parse(&input.url).map_err(invalid)?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
                || input.url.len() > 8192
                || input.body.len() > 1024 * 1024
                || input.headers.len() > 64
                || input
                    .headers
                    .iter()
                    .map(|(key, value)| key.len() + value.len())
                    .sum::<usize>()
                    > 64 * 1024
            {
                return Err(invalid("invalid HTTP request or request limit exceeded"));
            }
            let mut headers = HeaderMap::new();
            let destination = maka_sandbox::Destination::new(
                url.host_str().ok_or_else(|| invalid("missing HTTP host"))?,
                url.port_or_known_default()
                    .ok_or_else(|| invalid("missing HTTP port"))?,
            )
            .map_err(invalid)?;
            for (name, value) in input.headers {
                let name = HeaderName::from_bytes(name.as_bytes()).map_err(invalid)?;
                if matches!(
                    name.as_str(),
                    "host"
                        | "connection"
                        | "content-length"
                        | "transfer-encoding"
                        | "upgrade"
                        | "proxy-authorization"
                        | "proxy-connection"
                ) {
                    return Err(invalid("HTTP framing and proxy headers are owned by Host"));
                }
                headers.append(name, HeaderValue::from_str(&value).map_err(invalid)?);
            }
            if call.identity.agent().is_some() {
                let granted = self
                    .permissions
                    .request(
                        call.clone(),
                        maka_plugins::permissions::Request {
                            reason: format!(
                                "Allow plugin HTTP access to {}",
                                url.origin().ascii_serialization()
                            ),
                            permissions: maka_plugins::permissions::Permissions {
                                filesystem: Vec::new(),
                                network: maka_sandbox::Network::destination(destination.clone()),
                            },
                        },
                    )
                    .await
                    .map_err(|error| match error {
                        ToolError::Io {
                            kind: std::io::ErrorKind::PermissionDenied,
                            ..
                        } => Error::Denied,
                        other => failed(other),
                    })?;
                if !granted.network.allows(&destination) {
                    return Err(Error::Denied);
                }
            }
            let policy = host
                .plugin_network_policy(&call, &destination)
                .await
                .map_err(|_| Error::Denied)?;
            let client = {
                let mut cached = self.client.lock().await;
                if cached
                    .as_ref()
                    .is_none_or(|(previous, _)| previous != &policy)
                {
                    let client = policy
                        .client_builder()
                        .read_timeout(Duration::from_secs(60))
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .map_err(failed)?;
                    *cached = Some((policy, client));
                }
                cached.as_ref().unwrap().1.clone()
            };
            let method = match input.method {
                Method::Get => reqwest::Method::GET,
                Method::Head => reqwest::Method::HEAD,
                Method::Post => reqwest::Method::POST,
                Method::Put => reqwest::Method::PUT,
                Method::Patch => reqwest::Method::PATCH,
                Method::Delete => reqwest::Method::DELETE,
                Method::Options => reqwest::Method::OPTIONS,
            };
            let operation = maka_event_log::effects::Operation::Http {
                method: input.method,
                url: url.to_string(),
                body_digest: maka_runtime::artifact::content_digest(&input.body),
                body_bytes: input.body.len(),
            };
            let request = client
                .request(method, url)
                .headers(headers)
                .body(input.body);
            let mut ticket = call.resources.reserve().map_err(failed)?;
            let (head, response) = oneshot::channel();
            let (send, output) = mpsc::channel(1);
            let (ended, finished) = watch::channel(None);
            let stop = CancellationToken::new();
            let body = Arc::new(ResponseBody {
                host: self.host.clone(),
                owner: self.owner.clone(),
                call: call.clone(),
                destination: destination.clone(),
                stop: stop.clone(),
                output: tokio::sync::Mutex::new(output),
                ended: finished,
                _capacity: permit,
            });
            let owner = self.owner.clone();
            self.owner.spawn_resource("HTTP response", move |retiring| async move {
                ticket.start();
                let cancellation = call.cancellation.clone();
                let effect = Box::pin(async move {
                    tokio::select! {
                        biased;
                        _ = stop.cancelled() => Err("HTTP response closed".into()),
                        _ = retiring.cancelled() => Err("plugin retired during HTTP response".into()),
                        _ = cancellation.cancelled() => Err("invocation closed".into()),
                        result = worker::run(request, head, send) => result,
                    }.map_err(ToolError::OutcomeUnknown)
                });
                let result = host.record_plugin_http(owner, call, operation, destination, effect).await;
                // Cancelling local I/O cannot roll back remote side effects.
                let settled = match &result {
                    Err(error @ (ToolError::Persistence(_) | ToolError::CleanupUnconfirmed(_))) =>
                        Err(error.to_string()),
                    _ => Ok(()),
                };
                ticket.complete(settled.clone());
                ended.send_replace(Some(result.map(|_| ()).map_err(|error| error.to_string())));
                settled
            }).map_err(|_| Error::Denied)?;
            match response.await {
                Ok(Ok(head)) => Ok(Response { head, body }),
                result => {
                    body.close().await?;
                    Err(failed(match result {
                        Ok(Err(error)) => error,
                        _ => "HTTP request interrupted; remote outcome may be unknown".into(),
                    }))
                }
            }
        })
    }
}
struct ResponseBody {
    host: Weak<Executions>,
    owner: Context,
    call: Scope,
    destination: maka_sandbox::Destination,
    stop: CancellationToken,
    output: tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>,
    ended: watch::Receiver<Option<Result<(), String>>>,
    _capacity: tokio::sync::OwnedSemaphorePermit,
}
impl Drop for ResponseBody {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Body for ResponseBody {
    fn next(&self) -> BoxFuture<'_, Result<Option<Vec<u8>>, Error>> {
        Box::pin(async move {
            let _lease = self.owner.admit().map_err(|_| Error::Denied)?;
            if self.call.cancellation.is_cancelled() {
                return Err(Error::Denied);
            }
            self.host
                .upgrade()
                .ok_or(Error::Denied)?
                .check_plugin_network(&self.call, &self.destination)
                .await
                .map_err(|_| Error::Denied)?;
            let mut output = self
                .output
                .try_lock()
                .map_err(|_| invalid("HTTP response already has a reader"))?;
            tokio::select! {
                biased;
                _ = self.call.cancellation.cancelled() => Err(Error::Denied),
                _ = self.stop.cancelled() => Err(Error::Denied),
                chunk = output.recv() => match chunk {
                    Some(bytes) => Ok(Some(bytes)),
                    None => {
                        let mut ended = self.ended.clone();
                        ended.wait_for(Option::is_some).await.map_err(failed)?;
                        let result = ended.borrow().as_ref().unwrap().clone();
                        result.map(|()| None).map_err(failed)
                    }
                },
            }
        })
    }
    fn cancel(&self) {
        self.stop.cancel();
    }
    fn close(&self) -> BoxFuture<'_, Result<(), Error>> {
        self.cancel();
        Box::pin(async move {
            let mut ended = self.ended.clone();
            tokio::time::timeout(Duration::from_secs(5), ended.wait_for(Option::is_some))
                .await
                .map_err(|_| Error::CleanupUnconfirmed)?
                .map_err(|_| Error::CleanupUnconfirmed)?;
            Ok(())
        })
    }
}
fn invalid(error: impl ToString) -> Error {
    Error::Invalid(error.to_string())
}
fn failed(error: impl ToString) -> Error {
    Error::Failed(error.to_string())
}
fn message(error: impl ToString) -> String {
    error.to_string()
}
