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

mod credentials;
use futures_util::future::BoxFuture;
use maka_event_log::{EventLog, StoreError as LogError};
use maka_plugins::{
    fiber::Context,
    storage::{Mutation, Namespace, Record, Store, StoreError},
};
use std::sync::Arc;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub(crate) struct BoundStore {
    log: Arc<EventLog>,
    configuration: Arc<maka_config::ConfigurationStore>,
    context: Context,
    namespace: Namespace,
    workers: TaskTracker,
    shutdown: CancellationToken,
}

impl BoundStore {
    pub(crate) fn new(
        log: Arc<EventLog>,
        configuration: Arc<maka_config::ConfigurationStore>,
        context: Context,
        workers: TaskTracker,
        shutdown: CancellationToken,
    ) -> Result<Self, maka_plugins::Error> {
        let identity = context.identity()?;
        Ok(Self {
            log,
            configuration,
            workers,
            shutdown,
            namespace: Namespace::new(identity.package_id, identity.scope)?,
            context,
        })
    }
}

impl Store for BoundStore {
    fn read(&self, key: String) -> BoxFuture<'_, Result<Option<Record>, StoreError>> {
        Box::pin(async move {
            if self.shutdown.is_cancelled() {
                return Err(StoreError::Retired);
            }
            let _lease = self
                .context
                .resource_call()
                .map_err(|_| StoreError::Retired)?;
            self.log
                .plugin_data(&self.namespace, &key)
                .await
                .map_err(storage)
        })
    }

    fn batch(&self, mutations: Vec<Mutation>) -> BoxFuture<'_, Result<Vec<Record>, StoreError>> {
        Box::pin(async move {
            if self.shutdown.is_cancelled() {
                return Err(StoreError::Retired);
            }
            let lease = self
                .context
                .resource_call()
                .map_err(|_| StoreError::Retired)?;
            let log = self.log.clone();
            let namespace = self.namespace.clone();
            let shutdown = self.shutdown.clone();
            let (send, receive) = tokio::sync::oneshot::channel();
            // Retirement waits for the actual commit outcome, not the response
            // receiver. A lost reply never means the write was rolled back.
            self.workers.spawn(async move {
                let result = log
                    .plugin_data_batch(&namespace, mutations)
                    .await
                    .map_err(storage);
                if matches!(result, Err(StoreError::OutcomeUnknown(_))) {
                    shutdown.cancel();
                }
                drop(lease);
                let _ = send.send(result);
            });
            receive
                .await
                .map_err(|_| StoreError::OutcomeUnknown("storage owner disappeared".into()))?
        })
    }
}

fn storage(error: LogError) -> StoreError {
    match error {
        LogError::RevisionConflict { expected, actual } => {
            StoreError::Conflict { expected, actual }
        }
        LogError::CommitUnknown(_) | LogError::OperationUnknown => {
            StoreError::OutcomeUnknown(error.to_string())
        }
        other => StoreError::Unavailable(other.to_string()),
    }
}
