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

use super::{Context, Effect};
use crate::Error;
use futures_util::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

impl Context {
    /// Business tasks cannot run until the instance's publication is effective.
    /// Dropping the returned receiver does not detach the task from its Fiber.
    pub fn spawn(
        &self,
        label: impl Into<String>,
        task: impl Future<Output = Result<(), String>> + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<(), String>>, Error> {
        let context = self.clone();
        self.spawn_owned(label, async move {
            context
                .effective()
                .await
                .map_err(|error| error.to_string())?;
            task.await
        })
    }

    pub(crate) fn spawn_owned<T: Send + 'static>(
        &self,
        label: impl Into<String>,
        task: impl Future<Output = Result<T, String>> + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<T, String>>, Error> {
        self.spawn_task(label, task, None)
    }

    /// Own a resource worker whose cancellation must be signalled, never aborted.
    /// Success confirms cleanup. An error fences the instance against replacement.
    pub fn spawn_resource<T, F>(
        &self,
        label: impl Into<String>,
        task: impl FnOnce(CancellationToken) -> F,
    ) -> Result<oneshot::Receiver<Result<T, String>>, Error>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, String>> + Send + 'static,
    {
        let stopping = self.stopping()?.child_token();
        let future = task(stopping.clone());
        self.spawn_task(label, future, Some(stopping))
    }

    fn spawn_task<T: Send + 'static>(
        &self,
        label: impl Into<String>,
        task: impl Future<Output = Result<T, String>> + Send + 'static,
        stopping: Option<CancellationToken>,
    ) -> Result<oneshot::Receiver<Result<T, String>>, Error> {
        let inner = self.inner.upgrade().ok_or(Error::Retired)?;
        let resource = stopping.is_some();
        let (send, receive) = oneshot::channel();
        // Transfer ownership before polling user code, including on a multithread runtime.
        let (start, started) = oneshot::channel();
        let context = self.clone();
        let handle = inner.runtime.spawn(async move {
            let Ok(id) = started.await else {
                return;
            };
            let result = AssertUnwindSafe(task)
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err("plugin task panicked".into()));
            if resource && let Err(error) = &result {
                context.cleanup_failed(error.clone());
            }
            let _ = send.send(result);
            context.task_completed(id);
        });
        let abort = handle.abort_handle();
        let effect = Effect::new(
            label,
            move || match stopping {
                Some(token) => token.cancel(),
                None => abort.abort(),
            },
            move || async move {
                match handle.await {
                    Ok(()) => Ok(()),
                    Err(error) if error.is_cancelled() => Ok(()),
                    Err(error) => Err(error.to_string()),
                }
            },
        );
        let id = match self.own_effect(effect) {
            Ok(id) => id,
            Err(effect) => {
                // The task never received start, hence has acquired no plugin resources.
                drop(effect);
                return Err(Error::Retired);
            }
        };
        let _ = start.send(id);
        Ok(receive)
    }
}

#[cfg(test)]
mod tests {
    use crate::{composition::Scope, fiber::Fiber};

    #[tokio::test]
    async fn completed_tasks_release_owner_records_without_waiting_for_unload() {
        let fiber = Fiber::new("example", "example", Scope::Profile).unwrap();
        fiber.begin_loading().unwrap();
        fiber.ready().unwrap();
        fiber.publish().unwrap();
        let context = fiber.context();
        for _ in 0..32 {
            context
                .spawn("short task", async { Ok(()) })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
        }
        tokio::task::yield_now().await;
        assert!(fiber.inner.state.lock().unwrap().effects.is_empty());
        fiber
            .shutdown(tokio::time::Instant::now() + std::time::Duration::from_secs(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn resource_workers_receive_stop_and_acknowledge_cleanup_before_fiber_disposal() {
        for failure in [false, true] {
            let fiber = Fiber::new("resource", "resource", Scope::Profile).unwrap();
            fiber.begin_loading().unwrap();
            fiber.ready().unwrap();
            fiber.publish().unwrap();
            let settled = fiber
                .context()
                .spawn_resource("resource", move |stop| async move {
                    stop.cancelled().await;
                    tokio::task::yield_now().await;
                    if failure {
                        Err("cleanup unconfirmed".into())
                    } else {
                        Ok(42)
                    }
                })
                .unwrap();
            fiber.retire();
            let result = settled.await.expect("resource worker must not be aborted");
            assert_eq!(result.is_err(), failure);
            let result = fiber
                .shutdown(tokio::time::Instant::now() + std::time::Duration::from_secs(1))
                .await;
            assert_eq!(result.is_err(), failure);
            assert_eq!(
                fiber.phase(),
                if failure {
                    crate::fiber::Phase::Failed
                } else {
                    crate::fiber::Phase::Disposed
                }
            );
        }
    }
}
