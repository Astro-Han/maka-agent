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

use super::{Executions, Result, failure, internal};
use maka_protocol::OperationErrorCode as Code;
pub(crate) use maka_skills::plugin::{
    InputPreparation as SkillPreparation, Snapshot as FrozenSkills,
};
use std::{collections::HashSet, sync::Arc};

pub(crate) fn skill_error(error: maka_skills::plugin::Error) -> maka_protocol::OperationError {
    let code = match error {
        maka_skills::plugin::Error::Retired => Code::OperationUnavailable,
        maka_skills::plugin::Error::OutcomeUnknown(_) => Code::CommitOutcomeUnknown,
        maka_skills::plugin::Error::Source(_) => Code::PersistenceFailed,
        maka_skills::plugin::Error::InputTooLarge => Code::OperationConflict,
        maka_skills::plugin::Error::Invalid(_) => Code::InvalidRequest,
        _ => Code::InternalFailure,
    };
    failure(code, &error.to_string())
}

impl Executions {
    pub(crate) fn skills(
        &self,
    ) -> Option<maka_plugins::contributions::Contribution<maka_skills::plugin::Skills>> {
        self.plugin_catalog
            .snapshot::<maka_skills::plugin::Skills>(&maka_plugins::composition::Scope::Profile)
            .entries
            .remove(maka_skills::plugin::ID)
    }

    /// Query-only capability selection; it never binds or creates a Session.
    pub(crate) async fn preview_skills(
        &self,
        session_id: Option<&str>,
        connection_id: uuid::Uuid,
        cwd: &str,
        mode: maka_protocol::session::PermissionMode,
        profile: Option<maka_protocol::session::SessionToolProfile>,
    ) -> Result<Arc<FrozenSkills>> {
        let tools = self
            .preview_tool_catalog(session_id, connection_id, cwd, mode, profile)
            .await?
            .resolve_plugins()
            .map_err(internal)?;
        let skills = self
            .load_skills(cwd, tools.names().into_iter().collect())
            .await?;
        Ok(Arc::new(skills))
    }

    pub(crate) async fn load_skills(
        &self,
        cwd: &str,
        tools: HashSet<String>,
    ) -> Result<FrozenSkills> {
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        if !tools.contains("Skill") {
            return Ok(FrozenSkills::empty());
        }
        match self.skills() {
            Some(skills) => skills.value.capture(cwd, tools).await.map_err(skill_error),
            None => Ok(FrozenSkills::empty()),
        }
    }
}
