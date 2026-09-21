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

use super::callbacks::{Callback, invoke};
use maka_plugins::{background::BackgroundWork, fiber::Context};
use maka_runtime::tools::ToolError;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

struct Pending {
    wake: Arc<Notify>,
    stopped: CancellationToken,
}

impl BackgroundWork for Pending {
    fn is_pending(&self) -> bool {
        !self.stopped.is_cancelled()
    }

    fn wake(&self) {
        self.wake.notify_one();
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.stopped.cancel();
    }
}

pub(super) fn pending(
    context: &Context,
    callback: Arc<Callback>,
) -> Result<Arc<dyn BackgroundWork>, String> {
    let stopped = context.stopping().map_err(super::message)?.child_token();
    let wake = Arc::new(Notify::new());
    let pending = Arc::new(Pending {
        wake: wake.clone(),
        stopped: stopped.clone(),
    });
    let owner = context.clone();
    context
        .spawn_resource("JavaScript background wake", move |_| async move {
            tokio::select! {
                biased;
                _ = stopped.cancelled() => return Ok(()),
                result = owner.effective() => if result.is_err() { return Ok(()); },
            }
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.cancelled() => return Ok(()),
                    _ = wake.notified() => {}
                }
                match invoke(
                    &callback.module,
                    callback.id,
                    Value::Null,
                    Value::Null,
                    stopped.clone(),
                )
                .await
                {
                    Err(ToolError::CleanupUnconfirmed(error)) => return Err(error),
                    Err(error) if !stopped.is_cancelled() => {
                        eprintln!("plugin background wake failed: {error}");
                    }
                    _ => {}
                }
            }
        })
        .map_err(super::message)?;
    Ok(pending)
}
