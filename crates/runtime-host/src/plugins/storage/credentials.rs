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

use super::BoundStore;
use futures_util::future::BoxFuture;
use maka_plugins::{
    credentials::{Credentials, Record, Write, WriteResult},
    storage::StoreError,
};
impl Credentials for BoundStore {
    fn read(&self, key: String) -> BoxFuture<'_, Result<Option<Record>, StoreError>> {
        Box::pin(async move {
            if self.shutdown.is_cancelled() {
                return Err(StoreError::Retired);
            }
            let _lease = self
                .context
                .resource_call()
                .map_err(|_| StoreError::Retired)?;
            self.configuration
                .plugin_credential(self.namespace.clone(), key)
                .await
                .map_err(error)
        })
    }
    fn write(&self, input: Write) -> BoxFuture<'_, Result<WriteResult, StoreError>> {
        Box::pin(async move {
            if self.shutdown.is_cancelled() {
                return Err(StoreError::Retired);
            }
            let lease = self
                .context
                .resource_call()
                .map_err(|_| StoreError::Retired)?;
            let configuration = self.configuration.clone();
            let namespace = self.namespace.clone();
            let shutdown = self.shutdown.clone();
            let (send, receive) = tokio::sync::oneshot::channel();
            self.workers.spawn(async move {
                let result = configuration
                    .write_plugin_credential(namespace, input)
                    .await
                    .map_err(error);
                if matches!(result, Err(StoreError::OutcomeUnknown(_))) {
                    shutdown.cancel();
                }
                drop(lease);
                let _ = send.send(result);
            });
            receive
                .await
                .map_err(|_| StoreError::OutcomeUnknown("credential owner disappeared".into()))?
        })
    }
}
fn error(error: maka_config::ConfigError) -> StoreError {
    match error {
        maka_config::ConfigError::CommitUnknown => {
            StoreError::OutcomeUnknown("credential commit".into())
        }
        error => StoreError::Unavailable(error.to_string()),
    }
}
