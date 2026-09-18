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

use crate::{EventLog, StoreError};
use futures_util::future::BoxFuture;
use maka_graph::{
    Error, GraphId, WorkId,
    control::{Control, Intent, Wake},
    schedule::{CommittedUpdate, Update},
    store::Store,
};

impl Store for EventLog {
    fn commit_wake(&self, wake: Wake) -> BoxFuture<'_, Result<Wake, Error>> {
        Box::pin(async move { self.commit_graph_wake(wake).await.map_err(error) })
    }
    fn control(&self, root: &str, graph: &GraphId) -> BoxFuture<'_, Result<Control, Error>> {
        let root = root.to_owned();
        let graph = graph.clone();
        Box::pin(async move {
            self.graph_control(&root, Some(&graph))
                .await
                .map_err(error)?
                .ok_or_else(|| Error::NotFound(graph.to_string()))
        })
    }
    fn updates(
        &self,
        graph: &GraphId,
        after: u64,
        through: u64,
    ) -> BoxFuture<'_, Result<Vec<CommittedUpdate>, Error>> {
        let graph = graph.clone();
        Box::pin(async move {
            self.graph_updates(&graph, after, through)
                .await
                .map_err(error)
        })
    }
    fn commit_update(
        &self,
        update: Update,
        expected: u64,
        now: u64,
    ) -> BoxFuture<'_, Result<CommittedUpdate, Error>> {
        Box::pin(async move {
            self.commit_graph_update(update, expected, now)
                .await
                .map_err(error)
        })
    }
    fn intent(
        &self,
        graph: &GraphId,
        work: &WorkId,
    ) -> BoxFuture<'_, Result<Option<Intent>, Error>> {
        let (graph, work) = (graph.clone(), work.clone());
        Box::pin(async move { self.graph_intent(&graph, &work).await.map_err(error) })
    }
    fn commit_intent(&self, intent: Intent) -> BoxFuture<'_, Result<Intent, Error>> {
        Box::pin(async move { self.commit_graph_intent(intent).await.map_err(error) })
    }
}
fn error(error: StoreError) -> Error {
    match error {
        StoreError::EventConflict => Error::Conflict,
        StoreError::Sealed => Error::Closed,
        error => Error::Persistence(error.to_string()),
    }
}
