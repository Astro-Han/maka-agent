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

//! Host-issued capabilities, bound before either native or JS activation.

use crate::{credentials::Credentials, execution::Access, fiber::Context, storage::Store};
use futures_util::future::BoxFuture;
use std::sync::Arc;

#[derive(Clone)]
pub struct Services {
    pub inputs: crate::filesystem::ReadInputs,
    pub preferences: Arc<dyn crate::preferences::Preferences>,
    pub storage: Arc<dyn Store>,
    pub credentials: Arc<dyn Credentials>,
    pub executions: Arc<dyn Access>,
    pub sessions: Arc<dyn crate::session::catalog::Queries>,
    pub history: Arc<dyn crate::session::history::Queries>,
    pub authorizations: Arc<dyn crate::authorization::Access>,
    pub permissions: Arc<dyn crate::permissions::Access>,
    pub files: Arc<dyn crate::filesystem::Files>,
    pub models: Arc<dyn crate::llm::Models>,
    pub clients: Arc<dyn crate::client_capability::Clients>,
    pub http: Arc<dyn crate::http::Client>,
    pub processes: Arc<dyn crate::process::Processes>,
    pub terminals: Arc<dyn crate::terminal::Terminals>,
}

/// Embedding boundary. Kernel calls the same issuer for every package; plugins
/// receive only the resulting scoped services, never the issuer or full Host.
pub trait Provider: Send + Sync {
    fn bind(&self, owner: Context) -> BoxFuture<'_, Result<Option<Services>, String>>;
}
