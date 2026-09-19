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
use maka_plugins::remote::{Caller, Error, Stream, StreamProvider};
use serde_json::Value;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

pub(super) struct Provider(pub watch::Sender<()>);
impl StreamProvider for Provider {
    fn open(
        &self,
        input: Value,
        caller: Caller,
    ) -> BoxFuture<'static, Result<Box<dyn Stream>, Error>> {
        let changed = self.0.subscribe();
        Box::pin(async move {
            if !input.is_null() {
                return Err(Error::Invalid("Changes takes no arguments".into()));
            }
            Ok(Box::new(Changes {
                state: Mutex::new((changed, true)),
                stop: caller.cancellation.child_token(),
            }) as Box<dyn Stream>)
        })
    }
}
struct Changes {
    state: Mutex<(watch::Receiver<()>, bool)>,
    stop: CancellationToken,
}
impl Stream for Changes {
    fn next(&self) -> BoxFuture<'_, Result<Option<Value>, Error>> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if self.stop.is_cancelled() {
                return Ok(None);
            }
            if state.1 {
                state.1 = false;
                state.0.borrow_and_update();
                return Ok(Some(Value::Null));
            }
            tokio::select! {
                biased;
                _ = self.stop.cancelled() => Ok(None),
                changed = state.0.changed() => {
                    changed.map_err(|_| Error::Retired)?;
                    Ok(Some(Value::Null))
                }
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
