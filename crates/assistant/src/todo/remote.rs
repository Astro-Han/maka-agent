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

use super::{Item, Repository, message};
use futures_util::future::BoxFuture;
use maka_plugins::{
    client::Bundle,
    contributions::Staged,
    remote::{Caller, Endpoint, Error, Handler, Stream, StreamProvider, key},
};
use serde::Serialize;
use serde_json::Value;
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

pub(super) fn publish(
    repository: Arc<Repository>,
    package: &str,
    bundle: &Bundle,
    staged: &mut Staged,
) -> Result<(), String> {
    staged
        .insert(
            key(package, "watch").map_err(message)?,
            Endpoint::new(
                bundle.content_digest.clone(),
                Handler::Stream(Arc::new(Provider(repository))),
            ),
        )
        .map_err(message)
}
struct Provider(Arc<Repository>);
impl StreamProvider for Provider {
    fn open(
        &self,
        input: Value,
        caller: Caller,
    ) -> BoxFuture<'static, Result<Box<dyn Stream>, Error>> {
        let repository = self.0.clone();
        let changed = repository.changed.subscribe();
        Box::pin(async move {
            if !input.is_null() {
                return Err(Error::Invalid("Todo watch takes no arguments".into()));
            }
            let session = caller
                .session_id
                .clone()
                .ok_or_else(|| Error::Invalid("Todo requires a Session target".into()))?;
            caller.views.session().await?;
            let stop = caller.cancellation.child_token();
            Ok(Box::new(Watch {
                repository,
                session,
                caller,
                stop,
                state: Mutex::new(State {
                    changed,
                    revision: None,
                    pending: VecDeque::new(),
                }),
            }) as Box<dyn Stream>)
        })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    revision: Option<u64>,
    offset: usize,
    total: usize,
    items: Vec<Item>,
}
struct State {
    changed: watch::Receiver<()>,
    // Outer None is the first read; an absent document has revision Some(None).
    revision: Option<Option<u64>>,
    pending: VecDeque<Page>,
}
struct Watch {
    repository: Arc<Repository>,
    session: String,
    caller: Caller,
    stop: CancellationToken,
    state: Mutex<State>,
}
impl Stream for Watch {
    fn next(&self) -> BoxFuture<'_, Result<Option<Value>, Error>> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            loop {
                if self.stop.is_cancelled() {
                    return Ok(None);
                }
                if state.pending.is_empty() {
                    if state.revision.is_some() {
                        tokio::select! {
                            biased;
                            _ = self.stop.cancelled() => return Ok(None),
                            changed = state.changed.changed() => changed.map_err(|_| Error::Retired)?,
                        }
                    } else {
                        state.changed.borrow_and_update();
                    }
                    self.caller.views.session().await?;
                    let snapshot = self
                        .repository
                        .read(&self.session)
                        .await
                        .map_err(|error| Error::Provider(error.to_string()))?;
                    if state.revision == Some(snapshot.revision) {
                        continue;
                    }
                    state.revision = Some(snapshot.revision);
                    let total = snapshot.document.items.len();
                    for (index, items) in snapshot.document.items.chunks(32).enumerate() {
                        state.pending.push_back(Page {
                            revision: snapshot.revision,
                            offset: index * 32,
                            total,
                            items: items.to_vec(),
                        });
                    }
                    if total == 0 {
                        state.pending.push_back(Page {
                            revision: snapshot.revision,
                            offset: 0,
                            total,
                            items: vec![],
                        });
                    }
                }
                // Re-check current access even when returning a frozen continuation.
                self.caller.views.session().await?;
                let page = state
                    .pending
                    .pop_front()
                    .expect("snapshot has at least one page");
                return serde_json::to_value(page)
                    .map(Some)
                    .map_err(|error| Error::Provider(error.to_string()));
            }
        })
    }
    fn cancel(&self) {
        self.stop.cancel();
    }
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), Error>> {
        self.stop.cancel();
        Box::pin(async { Ok(()) })
    }
}
