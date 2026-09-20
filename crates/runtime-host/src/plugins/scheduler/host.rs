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
use maka_plugins::{fiber::Context, storage::Store};
use maka_runtime::event::Invocation;
use maka_scheduler::{Error, delivery::Dispatcher, task::ExecutionTemplate};
use std::sync::Arc;

/// Activation receives only scoped storage and scheduling operations, never a
/// Host handle, SQL connection, configuration writer or admission lock.
pub(crate) trait Services: Send + Sync {
    fn open(&self, context: Context) -> Result<Opened, String>;
}

pub(crate) struct Opened {
    pub storage: Arc<dyn Store>,
    pub operations: Arc<dyn Operations>,
}

pub(crate) trait Operations: Dispatcher {
    fn template(&self, invocation: Invocation) -> BoxFuture<'_, Result<ExecutionTemplate, Error>>;
}
