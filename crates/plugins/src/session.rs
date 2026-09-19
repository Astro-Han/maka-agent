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
use std::{collections::BTreeSet, sync::Arc};

/// Session behavior prepares its scoped capabilities. It cannot modify an
/// already-frozen model request or replace Host execution authority.
pub trait Behavior: Send + Sync {
    fn prepare(&self, session_id: String) -> BoxFuture<'_, Result<Preparation, String>>;
}

pub struct SessionBehavior(pub Arc<dyn Behavior>);

#[derive(Default)]
pub struct Preparation {
    pub instructions: String,
    /// A behavior may select its invocation presentation; it never widens tools.
    pub tool_mode: Option<maka_runtime::execution::ToolMode>,
    pub native_tools: maka_runtime::execution::NativeToolSet,
    pub required_clients: Option<ClientTools>,
    pub tool_ceiling: Option<BTreeSet<String>>,
    /// A business epoch may close after preparation but before Host admission.
    pub admission: Option<tokio_util::sync::CancellationToken>,
}

/// Required tools must share one Session-bound provider. Optional tools are
/// exposed only when available; neither list can grant a Client capability.
pub struct ClientTools {
    pub required: Vec<String>,
    pub optional: Vec<String>,
    pub private: BTreeSet<String>,
}
impl Preparation {
    pub fn validate(&self) -> Result<(), crate::Error> {
        if let Some(clients) = &self.required_clients {
            if clients.required.is_empty() || clients.required.len() + clients.optional.len() > 128
            {
                return Err(crate::Error::Invalid(
                    "Invalid required Client tool selection".into(),
                ));
            }
            for name in clients.required.iter().chain(&clients.optional) {
                crate::name(name)?;
            }
            if clients
                .private
                .iter()
                .any(|name| !clients.required.contains(name) && !clients.optional.contains(name))
            {
                return Err(crate::Error::Invalid(
                    "Private Client tools must be bound by the behavior".into(),
                ));
            }
        }
        if self.instructions.len() > 16 * 1024
            || self
                .tool_ceiling
                .as_ref()
                .is_some_and(|names| names.len() > 128)
        {
            return Err(crate::Error::Invalid(
                "Session behavior surface exceeds its budget".into(),
            ));
        }
        if let Some(names) = &self.tool_ceiling {
            for name in names {
                crate::name(name)?;
            }
        }
        Ok(())
    }
}
