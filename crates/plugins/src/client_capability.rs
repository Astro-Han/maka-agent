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
    /// User/Background calls need an explicit Notifications grant. No delivery
    /// retry is implicit: an uncertain acknowledgement is a domain decision.
    /// Cancellation withdraws an unadmitted offer; an admitted delivery settles.
    fn notify(
        &self,
        call: crate::call::Scope,
        input: Notification,
    ) -> futures_util::future::BoxFuture<'_, Result<(), maka_runtime::tools::ToolError>>;
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

pub const NOTIFICATION_SERVICE: &str = "maka_notifications";
pub const NOTIFICATION_VERSION: &str = "1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub body: String,
    pub destination: Destination,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Destination {
    Local,
    Channel { channel: String, recipient: String },
}
impl Notification {
    pub fn validate(&self) -> Result<(), crate::Error> {
        crate::name(&self.id)?;
        if self.title.trim().is_empty() || self.title.len() > 512 || self.body.len() > 32 * 1024 {
            return Err(crate::Error::Invalid(
                "notification exceeds content limits".into(),
            ));
        }
        if let Destination::Channel { channel, recipient } = &self.destination {
            crate::identifier(channel)?;
            if recipient.trim().is_empty() || recipient.len() > 512 || recipient.contains('\0') {
                return Err(crate::Error::Invalid(
                    "invalid notification recipient".into(),
                ));
            }
        }
        Ok(())
    }
}
