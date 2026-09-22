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

//! Host-owned model I/O shared by native and JavaScript adapters.
use futures_util::{SinkExt, StreamExt, future::BoxFuture};
use maka_plugins::{
    http,
    model::{Connect, Error, Frame, Socket, Transport},
};
use maka_runtime::model::error::{ProviderFailure, ProviderFailureReason};
use std::{
    collections::{HashMap, hash_map::RandomState},
    hash::BuildHasher,
    sync::{Arc, Mutex},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

/// The pool owns connections; this handle admits I/O only while its call lives.
pub(super) struct Call {
    pub transport: Arc<dyn Transport>,
    pub events: Arc<dyn maka_plugins::model::Events>,
    pub cancellation: CancellationToken,
}
impl Transport for Call {
    fn identity(&self) -> u64 {
        self.transport.identity()
    }
    fn request(&self, request: http::Request) -> BoxFuture<'_, Result<http::Response, Error>> {
        Box::pin(async move {
            let response = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Err(Error::Cancelled),
                response = self.transport.request(request) => response?,
            };
            self.events.progress();
            Ok(http::Response {
                head: response.head,
                body: Arc::new(CallBody {
                    body: response.body,
                    events: self.events.clone(),
                    cancellation: self.cancellation.clone(),
                }),
            })
        })
    }
    fn connect(&self, request: Connect) -> BoxFuture<'_, Result<Arc<dyn Socket>, Error>> {
        Box::pin(async move {
            let socket = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Err(Error::Cancelled),
                socket = self.transport.connect(request) => socket?,
            };
            self.events.progress();
            // Accepted sockets belong to the adapter session and may outlive
            // this request. Each new call supplies its own cancellation context.
            Ok(socket)
        })
    }
}
struct CallBody {
    body: Arc<dyn http::Body>,
    events: Arc<dyn maka_plugins::model::Events>,
    cancellation: CancellationToken,
}
impl http::Body for CallBody {
    fn next(&self) -> BoxFuture<'_, Result<Option<Vec<u8>>, http::Error>> {
        Box::pin(async move {
            let result = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => return Err(http::Error::Denied),
                result = self.body.next() => result?,
            };
            self.events.progress();
            Ok(result)
        })
    }
    fn cancel(&self) {
        self.body.cancel();
    }
    fn close(&self) -> BoxFuture<'_, Result<(), http::Error>> {
        self.body.close()
    }
}

#[derive(Default)]
pub(super) struct Pool {
    routes: Mutex<HashMap<maka_network::Policy, Arc<Network>>>,
    identity: RandomState,
}
impl Pool {
    pub fn get(&self, policy: &maka_network::Policy) -> Result<Arc<dyn Transport>, Error> {
        let mut routes = self.routes.lock().unwrap();
        if let Some(network) = routes.get(policy) {
            return Ok(network.clone());
        }
        let network = Arc::new(Network {
            policy: policy.clone(),
            client: policy
                .client_builder()
                .build()
                .map_err(|_| Error::Adapter("model network initialization failed".into()))?,
            identity: self.identity.hash_one(policy),
        });
        // Settings edits cannot accumulate an unbounded pool of obsolete clients.
        if routes.len() >= 16 {
            routes.clear();
        }
        routes.insert(policy.clone(), network.clone());
        Ok(network)
    }
}
struct Network {
    policy: maka_network::Policy,
    client: reqwest::Client,
    identity: u64,
}
fn headers(values: Vec<(String, String)>) -> Result<reqwest::header::HeaderMap, Error> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in values {
        let name = name
            .parse::<reqwest::header::HeaderName>()
            .map_err(|_| Error::Adapter("invalid model HTTP header".into()))?;
        let bytes = value
            .chars()
            .map(u8::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| Error::Adapter("invalid model HTTP header".into()))?;
        let value = reqwest::header::HeaderValue::from_bytes(&bytes)
            .map_err(|_| Error::Adapter("invalid model HTTP header".into()))?;
        headers.append(name, value);
    }
    Ok(headers)
}
impl Transport for Network {
    fn identity(&self) -> u64 {
        self.identity
    }
    fn request(&self, request: http::Request) -> BoxFuture<'_, Result<http::Response, Error>> {
        Box::pin(async move {
            if request.body.len() > 32 * 1024 * 1024 {
                return Err(Error::Adapter("model HTTP input exceeds 32 MiB".into()));
            }
            let method = match request.method {
                http::Method::Get => reqwest::Method::GET,
                http::Method::Head => reqwest::Method::HEAD,
                http::Method::Post => reqwest::Method::POST,
                http::Method::Put => reqwest::Method::PUT,
                http::Method::Patch => reqwest::Method::PATCH,
                http::Method::Delete => reqwest::Method::DELETE,
                http::Method::Options => reqwest::Method::OPTIONS,
            };
            let response = self
                .client
                .request(method, request.url)
                .headers(headers(request.headers)?)
                .body(request.body)
                .send()
                .await
                .map_err(failure)?;
            let head = http::Head {
                status: response.status().as_u16(),
                url: response.url().to_string(),
                headers: response
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
                    .collect(),
            };
            Ok(http::Response {
                head,
                body: Arc::new(Body {
                    response: AsyncMutex::new(Some(response)),
                    cancelled: CancellationToken::new(),
                }),
            })
        })
    }
    fn connect(&self, request: Connect) -> BoxFuture<'_, Result<Arc<dyn Socket>, Error>> {
        Box::pin(async move {
            let mut url = reqwest::Url::parse(&request.url)
                .map_err(|_| Error::Adapter("invalid model WebSocket URL".into()))?;
            let scheme = match url.scheme() {
                "ws" | "http" => "http",
                "wss" | "https" => "https",
                _ => return Err(Error::Adapter("invalid model WebSocket scheme".into())),
            };
            url.set_scheme(scheme)
                .map_err(|_| Error::Adapter("invalid model WebSocket scheme".into()))?;
            let socket = maka_network::connect_websocket(
                self.policy.client_builder(),
                url.as_str(),
                headers(request.headers)?,
                8 * 1024 * 1024,
            )
            .await
            .map_err(|_| Error::Adapter("model WebSocket connection failed".into()))?;
            let (write, read) = socket.split();
            Ok(Arc::new(Connection {
                write: AsyncMutex::new(Some(write)),
                read: AsyncMutex::new(Some(read)),
                cancelled: CancellationToken::new(),
            }) as Arc<dyn Socket>)
        })
    }
}
struct Body {
    response: AsyncMutex<Option<reqwest::Response>>,
    cancelled: CancellationToken,
}
impl http::Body for Body {
    fn next(&self) -> BoxFuture<'_, Result<Option<Vec<u8>>, http::Error>> {
        Box::pin(async move {
            let mut response = self.response.lock().await;
            let Some(body) = response.as_mut() else {
                return Ok(None);
            };
            let result = tokio::select! {
                biased;
                _ = self.cancelled.cancelled() => return Err(http::Error::Denied),
                result = body.chunk() => result,
            };
            match result {
                Ok(Some(bytes)) if bytes.len() <= 8 * 1024 * 1024 => Ok(Some(bytes.to_vec())),
                Ok(Some(_)) => {
                    response.take();
                    Err(http::Error::Invalid(
                        "model HTTP chunk exceeds 8 MiB".into(),
                    ))
                }
                Ok(None) => {
                    response.take();
                    Ok(None)
                }
                Err(error) => {
                    response.take();
                    Err(match failure(error) {
                        Error::Provider(_) => {
                            http::Error::Failed("model HTTP stream interrupted".into())
                        }
                        _ => http::Error::Invalid("invalid model HTTP stream".into()),
                    })
                }
            }
        })
    }
    fn cancel(&self) {
        self.cancelled.cancel();
    }
    fn close(&self) -> BoxFuture<'_, Result<(), http::Error>> {
        self.cancel();
        Box::pin(async move {
            self.response.lock().await.take();
            Ok(())
        })
    }
}
type Wire = tokio_tungstenite::tungstenite::Message;
struct Connection {
    write: AsyncMutex<Option<futures_util::stream::SplitSink<maka_network::Socket, Wire>>>,
    read: AsyncMutex<Option<futures_util::stream::SplitStream<maka_network::Socket>>>,
    cancelled: CancellationToken,
}
impl Socket for Connection {
    fn send(&self, frame: Frame) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let frame = match frame {
                Frame::Text(text) => Wire::Text(text.into()),
                Frame::Binary(bytes) => Wire::Binary(bytes.into()),
            };
            if frame.len() > 8 * 1024 * 1024 {
                return Err(Error::Adapter("model WebSocket frame exceeds 8 MiB".into()));
            }
            let mut write = self.write.lock().await;
            let write = write.as_mut().ok_or(Error::Cancelled)?;
            tokio::select! {
                biased;
                _ = self.cancelled.cancelled() => Err(Error::Cancelled),
                result = write.send(frame) => result.map_err(|_| Error::Adapter("model WebSocket send failed".into())),
            }
        })
    }
    fn receive(&self) -> BoxFuture<'_, Result<Option<Frame>, Error>> {
        Box::pin(async move {
            let mut read = self.read.lock().await;
            let Some(read) = read.as_mut() else {
                return Ok(None);
            };
            loop {
                let next = tokio::select! {
                    biased;
                    _ = self.cancelled.cancelled() => return Err(Error::Cancelled),
                    next = read.next() => next,
                };
                match next {
                    Some(Ok(Wire::Text(text))) => return Ok(Some(Frame::Text(text.to_string()))),
                    Some(Ok(Wire::Binary(bytes))) => {
                        return Ok(Some(Frame::Binary(bytes.to_vec())));
                    }
                    Some(Ok(Wire::Ping(_))) => {
                        if let Some(write) = self.write.lock().await.as_mut() {
                            tokio::select! {
                                biased;
                                _ = self.cancelled.cancelled() => return Err(Error::Cancelled),
                                result = write.flush() => result.map_err(|_| {
                                    Error::Adapter("model WebSocket pong failed".into())
                                })?,
                            }
                        }
                    }
                    Some(Ok(Wire::Pong(_))) => {}
                    Some(Ok(Wire::Close(_))) | None => return Ok(None),
                    _ => return Err(Error::Adapter("model WebSocket receive failed".into())),
                }
            }
        })
    }
    fn close(&self) -> BoxFuture<'_, Result<(), Error>> {
        self.cancelled.cancel();
        Box::pin(async move {
            self.write.lock().await.take();
            self.read.lock().await.take();
            Ok(())
        })
    }
}
pub(super) fn failure(error: reqwest::Error) -> Error {
    use std::{error::Error as _, io::ErrorKind};
    let mut transient = error.is_timeout() || error.is_dns();
    let mut cause = error.source();
    while let Some(error) = cause {
        if let Some(error) = error.downcast_ref::<hyper::Error>() {
            transient |= error.is_closed() || error.is_incomplete_message();
        }
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            transient |= matches!(
                error.kind(),
                ErrorKind::ConnectionRefused
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::NotConnected
                    | ErrorKind::BrokenPipe
                    | ErrorKind::TimedOut
                    | ErrorKind::HostUnreachable
                    | ErrorKind::NetworkUnreachable
                    | ErrorKind::NetworkDown
            );
        }
        cause = error.source();
    }
    if transient {
        Error::Provider(ProviderFailure::new(
            ProviderFailureReason::Network,
            "model HTTP transport interrupted",
            true,
            None,
        ))
    } else {
        Error::Adapter("model HTTP request failed".into())
    }
}
