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

use crate::{
    Error, GraphId, WorkId,
    control::{Control, Intent, Wake},
    schedule::{CommittedUpdate, Update},
};
use futures_util::future::BoxFuture;

/// Graph policy sees durable decisions, not SQL connections or Host internals.
pub trait Store: Send + Sync {
    fn control(&self, root: &str, graph: &GraphId) -> BoxFuture<'_, Result<Control, Error>>;
    fn updates(
        &self,
        graph: &GraphId,
        after: u64,
        through: u64,
    ) -> BoxFuture<'_, Result<Vec<CommittedUpdate>, Error>>;
    fn commit_update(
        &self,
        update: Update,
        expected: u64,
        now: u64,
    ) -> BoxFuture<'_, Result<CommittedUpdate, Error>>;
    fn intent(
        &self,
        graph: &GraphId,
        work: &WorkId,
    ) -> BoxFuture<'_, Result<Option<Intent>, Error>>;
    fn commit_intent(&self, intent: Intent) -> BoxFuture<'_, Result<Intent, Error>>;
    fn commit_wake(&self, wake: Wake) -> BoxFuture<'_, Result<Wake, Error>>;
}
