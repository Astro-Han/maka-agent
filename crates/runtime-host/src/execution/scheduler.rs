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

mod authorization;
mod delivery;
mod notification;

use super::Executions;
use crate::plugins::scheduler::host::{Opened, Operations, Services};
use futures_util::future::BoxFuture;
use maka_plugins::{execution::CommandError, fiber::Context};
use maka_runtime::event::Invocation;
use maka_scheduler::{Error, task::ExecutionTemplate};
use std::sync::{Arc, Weak};

pub(crate) struct SchedulerServices {
    executions: Weak<Executions>,
    root_id: String,
}

impl SchedulerServices {
    pub(crate) fn new(executions: &Arc<Executions>, root_id: String) -> Arc<Self> {
        Arc::new(Self {
            executions: Arc::downgrade(executions),
            root_id,
        })
    }
}

impl Services for SchedulerServices {
    fn open(&self, context: Context) -> Result<Opened, String> {
        let host = self.executions.upgrade().ok_or("Host is closed")?;
        Ok(Opened {
            storage: host
                .plugin_store(context.clone())
                .map_err(|error| error.to_string())?,
            operations: Arc::new(Backend {
                context,
                executions: self.executions.clone(),
                root_id: self.root_id.clone(),
            }),
        })
    }
}

/// Only this Host adapter can resolve authorization or access native providers.
struct Backend {
    context: Context,
    executions: Weak<Executions>,
    root_id: String,
}

impl Backend {
    fn host(&self) -> Result<Arc<Executions>, CommandError> {
        self.executions
            .upgrade()
            .filter(|host| host.accepting())
            .ok_or(CommandError::Draining)
    }

    async fn privacy_allows(&self) -> Result<bool, Error> {
        self.host()
            .map_err(|error| Error::Unavailable(error.to_string()))?
            .configuration
            .runtime_policy()
            .await
            .map(|policy| !policy.policy.privacy.incognito_active)
            .map_err(|error| Error::Unavailable(error.to_string()))
    }
}

impl Operations for Backend {
    fn template(&self, invocation: Invocation) -> BoxFuture<'_, Result<ExecutionTemplate, Error>> {
        Box::pin(async move {
            let _lease = self
                .context
                .admit()
                .map_err(|error| Error::Unavailable(error.to_string()))?;
            let host = self
                .host()
                .map_err(|error| Error::Unavailable(error.to_string()))?;
            let frozen = host
                .log
                .invocation_configuration(&invocation)
                .await
                .map_err(|error| Error::Unavailable(error.to_string()))?
                .ok_or_else(|| Error::Invalid("unknown scheduling invocation".into()))?;
            let model = frozen
                .model
                .ok_or_else(|| Error::Invalid("agent_run requires a model target".into()))?;
            Ok(ExecutionTemplate {
                cwd: frozen.cwd,
                project_id: None,
                llm_connection_id: model.connection_id,
                llm_connection_slug: model.connection_slug,
                model: model.model,
                thinking_level: frozen.thinking_level,
                permission_mode: frozen.permission_mode,
                collaboration_mode: frozen.collaboration_mode,
                orchestration_mode: frozen.orchestration_mode,
            })
        })
    }
}
