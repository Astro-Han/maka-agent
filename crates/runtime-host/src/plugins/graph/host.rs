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
use maka_graph::{Epoch, GraphId, Mode, owner::Handle};
use maka_plugins::{
    contributions::Staged,
    execution::Commands,
    fiber::{Context, Fiber},
    storage::Store,
};
use maka_runtime::configuration::{ConnectionCatalogSnapshot, policy::SubagentPreset};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Only Host-coordinated transitions cross this port. No admission lock or
/// configuration writer is available to the Graph implementation.
pub(crate) trait Sessions: Send + Sync {
    fn activate(&self, request: Activation) -> BoxFuture<'_, Result<super::Root, String>>;
    fn retire_idle(
        &self,
        session: String,
        owner: Option<Context>,
    ) -> BoxFuture<'_, Result<bool, String>>;
    fn stop(
        &self,
        session: String,
        graph: GraphId,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<(), maka_plugins::remote::Error>>;
    fn preferences(&self) -> BoxFuture<'_, Result<Preferences, String>>;
}

pub(crate) struct Preferences {
    pub presets: Vec<SubagentPreset>,
    pub models: ConnectionCatalogSnapshot,
}
pub(crate) struct Opened {
    pub epoch: Epoch,
    pub commands: Arc<dyn Commands>,
    pub storage: Arc<dyn Store>,
}
pub(crate) struct Activation {
    pub session: String,
    pub mode: Mode,
    pub previous: Option<GraphId>,
    pub parent: Context,
    pub child: Fiber,
    pub stop: CancellationToken,
    /// Synchronous construction; Host publishes the complete staged surface
    /// before releasing admission. No plugin future runs under that lock.
    pub build: Build,
}

type Build = Box<dyn FnOnce(Opened) -> Result<(Handle, Staged), String> + Send>;
