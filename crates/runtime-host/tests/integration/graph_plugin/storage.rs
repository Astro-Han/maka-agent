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
use maka_event_log::EventLog;
use maka_plugins::{
    composition::Scope,
    storage::{Mutation, Namespace, Page, Record, Scan, Store, StoreError},
};
use std::sync::Arc;

/// Inspect and seed plugin-owned data with the same scoped store used by Host;
/// the canonical execution database has no Graph schema or business methods.
pub(super) fn repository(log: Arc<EventLog>) -> maka_graph::repository::Repository {
    maka_graph::repository::Repository::new(
        Arc::new(Storage {
            log,
            namespace: Namespace::new("maka.agent-graph", Scope::Profile).unwrap(),
        }),
        "maka.agent-graph",
    )
    .unwrap()
}
struct Storage {
    log: Arc<EventLog>,
    namespace: Namespace,
}
impl Store for Storage {
    fn read(&self, key: String) -> BoxFuture<'_, Result<Option<Record>, StoreError>> {
        Box::pin(async move {
            self.log
                .plugin_data(&self.namespace, &key)
                .await
                .map_err(error)
        })
    }
    fn scan(&self, query: Scan) -> BoxFuture<'_, Result<Page, StoreError>> {
        Box::pin(async move {
            self.log
                .plugin_data_scan(&self.namespace, query)
                .await
                .map_err(error)
        })
    }
    fn batch(&self, mutations: Vec<Mutation>) -> BoxFuture<'_, Result<Vec<Record>, StoreError>> {
        Box::pin(async move {
            self.log
                .plugin_data_batch(&self.namespace, mutations)
                .await
                .map_err(error)
        })
    }
}
fn error(error: maka_event_log::StoreError) -> StoreError {
    match error {
        maka_event_log::StoreError::RevisionConflict { expected, actual } => {
            StoreError::Conflict { expected, actual }
        }
        error => StoreError::Unavailable(error.to_string()),
    }
}
