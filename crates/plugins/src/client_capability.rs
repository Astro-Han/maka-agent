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

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub trait Clients: Send + Sync {
    fn tools(
        &self,
        call: crate::call::Scope,
    ) -> futures_util::future::BoxFuture<
        '_,
        Result<Vec<maka_runtime::tools::ToolDefinition>, maka_runtime::tools::ToolError>,
    >;
    fn call(
        &self,
        call: crate::call::Scope,
        input: Call,
    ) -> futures_util::future::BoxFuture<'_, Result<Value, maka_runtime::tools::ToolError>>;
}

/// A tool selected from the call scope's frozen Client Capability catalog.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub name: String,
    pub input: Map<String, Value>,
}
