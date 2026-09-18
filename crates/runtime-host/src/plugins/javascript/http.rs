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

use super::invocation::Authority;
use crate::execution::Executions;
use maka_plugins::fiber::Context;
use maka_runtime::event::Invocation;
use reqwest::{
    Client, Method,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub(super) enum Verb {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}
impl From<Verb> for Method {
    fn from(value: Verb) -> Self {
        match value {
            Verb::Get => Self::GET,
            Verb::Head => Self::HEAD,
            Verb::Post => Self::POST,
            Verb::Put => Self::PUT,
            Verb::Patch => Self::PATCH,
            Verb::Delete => Self::DELETE,
            Verb::Options => Self::OPTIONS,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    pub authority: String,
    pub url: String,
    pub method: Verb,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Vec<u8>,
}
#[derive(Serialize)]
pub(super) struct Head {
    pub handle: String,
    pub status: u16,
    pub url: String,
    // Bytes preserve non-UTF-8 field values and duplicate headers.
    pub headers: Vec<(String, Vec<u8>)>,
}
struct Handle {
    invocation: Invocation,
    expires: CancellationToken,
    stop: CancellationToken,
    output: tokio::sync::Mutex<mpsc::Receiver<Vec<u8>>>,
    ended: watch::Receiver<Option<Result<(), String>>>,
}
pub(super) struct Http {
    host: Weak<Executions>,
    owner: Context,
    client: tokio::sync::Mutex<Option<(maka_network::Policy, Client)>>,
    handles: Mutex<BTreeMap<String, Arc<Handle>>>,
}
impl Http {
    pub fn new(host: Weak<Executions>, owner: Context) -> Self {
        Self {
            host,
            owner,
            client: Default::default(),
            handles: Default::default(),
        }
    }
    pub async fn request(&self, authority: Authority, input: Request) -> Result<Head, String> {
        let _lease = self.owner.admit().map_err(message)?;
        let url = reqwest::Url::parse(&input.url).map_err(message)?;
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
            return Err("invalid HTTP request or request limit exceeded".into());
        }
        let mut headers = HeaderMap::new();
        for (name, value) in input.headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(message)?;
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
                return Err("HTTP framing and proxy headers are owned by Host".into());
            }
            headers.append(name, HeaderValue::from_str(&value).map_err(message)?);
        }
        let host = self.host.upgrade().ok_or("Host closed")?;
        let policy = host
            .plugin_network_policy(&authority.identity.invocation)
            .await
            .map_err(message)?;
        let client = {
            let mut cached = self.client.lock().await;
            if cached
                .as_ref()
                .is_none_or(|(previous, _)| previous != &policy)
            {
                let client = policy
                    .client_builder()
                    .read_timeout(Duration::from_secs(60))
                    // Request replay and redirect decisions belong to the plugin.
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(message)?;
                *cached = Some((policy, client));
            }
            cached.as_ref().unwrap().1.clone()
        };
        let request = client
            .request(input.method.into(), url)
            .headers(headers)
            .body(input.body);
        let mut ticket = authority.resources.reserve().map_err(message)?;
        let (head, response) = oneshot::channel();
        let (send, output) = mpsc::channel(1);
        let (ended, finished) = watch::channel(None);
        let stop = CancellationToken::new();
        let handle = Arc::new(Handle {
            invocation: authority.identity.invocation,
            expires: authority.cancellation.clone(),
            stop: stop.clone(),
            output: tokio::sync::Mutex::new(output),
            ended: finished,
        });
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut handles = self.handles.lock().unwrap();
            handles.retain(|_, handle| !handle.expires.is_cancelled());
            if handles.len() >= 32 {
                return Err("HTTP handle capacity exceeded; close unused responses".into());
            }
            if authority.cancellation.is_cancelled() {
                return Err("invocation closed".into());
            }
            handles.insert(id.clone(), handle.clone());
        }
        let request_id = id.clone();
        let execution = self
            .owner
            .spawn_resource("HTTP response", move |retiring| async move {
                ticket.start();
                let result = tokio::select! {
                    biased;
                    _ = stop.cancelled() => Err("HTTP response closed".into()),
                    _ = retiring.cancelled() => Err("plugin retired during HTTP response".into()),
                    _ = authority.cancellation.cancelled() => Err("invocation closed".into()),
                    result = worker::run(request_id, request, head, send) => result,
                };
                // Dropping the request/body cancels local I/O; it cannot roll back
                // a remote server's already accepted side effects.
                ticket.complete(Ok(()));
                ended.send_replace(Some(result));
                Ok(())
            });
        if let Err(error) = execution {
            self.handles.lock().unwrap().remove(&id);
            return Err(error.to_string());
        }
        match response.await {
            Ok(Ok(head)) => Ok(head),
            result => {
                self.close(&id).await?;
                Err(match result {
                    Ok(Err(error)) => error,
                    _ => "HTTP request interrupted; remote outcome may be unknown".into(),
                })
            }
        }
    }
    pub async fn next(&self, authority: &Authority, id: &str) -> Result<Option<Vec<u8>>, String> {
        let handle = self
            .handles
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("HTTP response closed")?;
        if handle.invocation != authority.identity.invocation || handle.expires.is_cancelled() {
            return Err("HTTP response belongs to an expired or different invocation".into());
        }
        self.host
            .upgrade()
            .ok_or("Host closed")?
            .plugin_process_workspace(&authority.identity.invocation)
            .await
            .map_err(message)?;
        let mut output = handle
            .output
            .try_lock()
            .map_err(|_| "HTTP response already has a reader")?;
        tokio::select! {
            biased;
            _ = authority.cancellation.cancelled() => Err("invocation closed".into()),
            _ = handle.stop.cancelled() => Err("HTTP response closed".into()),
            chunk = output.recv() => match chunk {
                Some(bytes) => Ok(Some(bytes)),
                None => {
                    let mut ended = handle.ended.clone();
                    ended.wait_for(Option::is_some).await.map_err(message)?;
                    let result = ended.borrow().as_ref().unwrap().clone();
                    result.map(|()| None)
                }
            },
        }
    }
    pub async fn close(&self, id: &str) -> Result<(), String> {
        let Some(handle) = self.handles.lock().unwrap().remove(id) else {
            return Ok(());
        };
        handle.stop.cancel();
        let mut ended = handle.ended.clone();
        tokio::time::timeout(Duration::from_secs(5), ended.wait_for(Option::is_some))
            .await
            .map_err(|_| "HTTP cleanup unconfirmed")?
            .map_err(message)?;
        Ok(())
    }
}
fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
