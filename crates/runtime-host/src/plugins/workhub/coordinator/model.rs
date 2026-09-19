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

use super::super::control::{Result, failure};
use crate::session::{SessionModel, mutation_projection};
use maka_protocol::{
    OperationErrorCode as Code,
    session::{
        SessionConfigurationUpdateInput, SessionModelTarget, SessionUpdateResult, ThinkingLevel,
    },
};
use maka_runtime::{configuration::Patch, workhub::COORDINATION_SESSION_ID};

pub(crate) struct Prepared {
    pub expected_revision: u64,
    pub model: SessionModel,
    pub thinking: Option<ThinkingLevel>,
    pub lock_connection: bool,
}

impl super::super::Control {
    pub(crate) async fn configure_model(
        &self,
        input: SessionConfigurationUpdateInput,
    ) -> Result<SessionUpdateResult> {
        if input.session_id != COORDINATION_SESSION_ID
            || input.patch.permission_mode.is_some()
            || input.patch.collaboration_mode.is_some()
            || input.patch.orchestration_mode.is_some()
        {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub model selection cannot change the Session boundary",
            ));
        }
        let _call = self
            .caller
            .admit()
            .map_err(|error| failure(Code::OperationUnavailable, error.to_string()))?;
        let current = self
            .commands
            .coordinator(self.caller.clone())
            .await?
            .ok_or_else(|| failure(Code::NotFound, "WorkHub Session has not been resolved"))?;
        if current.revision != input.expected_revision {
            return Ok(SessionUpdateResult::RevisionConflict {
                expected_revision: input.expected_revision,
                actual_revision: current.revision,
            });
        }
        let config = current.configuration;
        let thinking = match input.patch.thinking_level {
            Patch::Keep => config.thinking_level,
            Patch::Clear => None,
            Patch::Set(level) => Some(level),
        };
        let current = config
            .target
            .model()
            .ok_or_else(|| failure(Code::OperationConflict, "WorkHub requires a model target"))?;
        let lock_connection = input.patch.model_target.is_some();
        let target = input.patch.model_target.or_else(|| {
            (!input.patch.thinking_level.is_keep()).then(|| SessionModelTarget::Explicit {
                connection_id: current.connection_id.clone(),
                connection_slug: current.connection_slug.clone(),
                model: current.model.clone(),
            })
        });
        let model = match target {
            Some(target) => self.commands.resolve_model(target, thinking).await?,
            None => current.clone(),
        };
        self.commands
            .configure_coordinator(
                self.caller.clone(),
                Prepared {
                    expected_revision: input.expected_revision,
                    model,
                    thinking,
                    lock_connection,
                },
            )
            .await
            .map(mutation_projection)
    }
}
