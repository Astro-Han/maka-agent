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

use futures_util::future::BoxFuture;
use maka_plugins::{
    client::{Bundle, Client},
    composition::Scope,
    contributions::Staged,
    kernel::{Plugin, PluginContext},
    remote::{Caller, Endpoint, Error, Handler, Method, Stream, StreamProvider, key},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(super) struct State {
    pub(super) calls: AtomicUsize,
    pub(super) opening: AtomicUsize,
    pub(super) live: AtomicUsize,
    pub(super) reads: AtomicUsize,
}
struct Adapter(Arc<State>);
pub(super) struct Example {
    pub(super) bundle: Arc<Bundle>,
    pub(super) state: Arc<State>,
}
impl Plugin for Example {
    fn supports_scope(&self, _: &Scope) -> bool {
        true
    }
    fn activate(
        &self,
        context: PluginContext,
        _: Value,
    ) -> BoxFuture<'static, Result<Staged, String>> {
        let bundle = self.bundle.clone();
        let state = self.state.clone();
        Box::pin(async move {
            let identity = context.lifecycle.identity().unwrap();
            let mut staged = Staged::default();
            if identity.scope == Scope::DesktopUi {
                staged
                    .insert(
                        identity.entry_id,
                        Client {
                            bundle,
                            config: Value::Null,
                        },
                    )
                    .unwrap();
            } else {
                for (name, handler) in [
                    (
                        "echo",
                        Handler::Method(Arc::new(Adapter(state.clone())) as Arc<dyn Method>),
                    ),
                    (
                        "events",
                        Handler::Stream(Arc::new(Adapter(state.clone())) as Arc<dyn StreamProvider>),
                    ),
                ] {
                    staged
                        .insert(
                            key(&identity.package_id, name).unwrap(),
                            Endpoint::new(bundle.content_digest.clone(), handler),
                        )
                        .unwrap();
                }
            }
            Ok(staged)
        })
    }
}
impl Method for Adapter {
    fn call(&self, input: Value, caller: Caller) -> BoxFuture<'static, Result<Value, Error>> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            Ok(
                json!({"input":input,"client":caller.client_instance_id,"session":caller.session_id}),
            )
        })
    }
}
impl StreamProvider for Adapter {
    fn open(
        &self,
        input: Value,
        caller: Caller,
    ) -> BoxFuture<'static, Result<Box<dyn Stream>, Error>> {
        let state = self.0.clone();
        Box::pin(async move {
            state.opening.fetch_add(1, Ordering::SeqCst);
            if input == "late" {
                caller.cancellation.cancelled().await;
            }
            state.opening.fetch_sub(1, Ordering::SeqCst);
            state.live.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Events {
                state,
                stop: CancellationToken::new(),
                count: AtomicUsize::new(0),
            }) as Box<dyn Stream>)
        })
    }
}
struct Events {
    state: Arc<State>,
    stop: CancellationToken,
    count: AtomicUsize,
}
impl Stream for Events {
    fn next(&self) -> BoxFuture<'_, Result<Option<Value>, Error>> {
        Box::pin(async move {
            match self.count.fetch_add(1, Ordering::SeqCst) {
                0 => return Ok(Some(Value::Null)),
                1 => return Ok(Some(json!("ready"))),
                _ => {}
            }
            self.state.reads.fetch_add(1, Ordering::SeqCst);
            self.stop.cancelled().await;
            Err(Error::Cancelled)
        })
    }
    fn cancel(&self) {
        self.stop.cancel();
    }
    fn close(self: Box<Self>) -> BoxFuture<'static, Result<(), Error>> {
        Box::pin(async move {
            assert!(self.stop.is_cancelled(), "signal before awaiting cleanup");
            self.state.live.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
