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

use futures_util::{FutureExt, future::BoxFuture};
use std::{
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
};

type Stop = Box<dyn FnOnce() + Send>;
type Close = Box<dyn FnOnce() -> BoxFuture<'static, Result<(), String>> + Send>;

/// Stop is synchronous and idempotent; close acknowledges asynchronous cleanup.
#[must_use]
pub struct Effect {
    label: String,
    stop: Option<Stop>,
    close: Option<Close>,
}

impl Effect {
    pub(super) fn label(&self) -> &str {
        &self.label
    }
    /// Only an owned task that has finished executing may acknowledge itself.
    pub(super) fn completed(mut self) {
        self.stop = None;
        self.close = None;
    }

    pub fn new<F>(
        label: impl Into<String>,
        stop: impl FnOnce() + Send + 'static,
        close: impl FnOnce() -> F + Send + 'static,
    ) -> Self
    where
        F: Future<Output = Result<(), String>> + Send + 'static,
    {
        Self {
            label: label.into(),
            stop: Some(Box::new(stop)),
            close: Some(Box::new(move || Box::pin(close()))),
        }
    }

    pub(super) fn signal(&mut self) -> Result<(), String> {
        if let Some(stop) = self.stop.take() {
            catch_unwind(AssertUnwindSafe(stop))
                .map_err(|_| format!("{}: stop callback panicked", self.label))?;
        }
        Ok(())
    }

    pub async fn close(mut self) -> Result<(), String> {
        let stopped = self.signal();
        let close = self.close.take().expect("effect cleanup runs once");
        let closed = AssertUnwindSafe(async move { close().await })
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err("cleanup callback panicked".into()))
            .map_err(|error| format!("{}: {error}", self.label));
        match (stopped, closed) {
            (Err(stop), Err(close)) => Err(format!("{stop}; {close}")),
            (Err(error), _) | (_, Err(error)) => Err(error),
            _ => Ok(()),
        }
    }
}

impl Drop for Effect {
    fn drop(&mut self) {
        let _ = self.signal();
    }
}
