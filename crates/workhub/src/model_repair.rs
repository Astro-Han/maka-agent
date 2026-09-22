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
    Error,
    assignment::{Assignment, Assignments, Target},
    invalid,
    repository::digest,
};
use maka_plugins::{
    authorization,
    execution::{
        CommandError, Commands, Configure, Configured, CreateRoot, Target as ExecutionTarget,
    },
    llm::Selection,
    session::View,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Choice {
    pub revision: Option<u64>,
    pub authorization: authorization::Target,
    pub name: String,
    pub target: Option<ExecutionTarget>,
}

#[derive(Serialize, Deserialize)]
struct Selected {
    id: uuid::Uuid,
    target: ExecutionTarget,
}

impl Assignments {
    pub(super) async fn model_choice(&self, operation: &str) -> Result<Choice, Error> {
        let assignment = self.model_assignment(operation).await?;
        let choice = self
            .repository
            .read::<Selected>(&choice_key(&assignment.request.target)?)
            .await?;
        let (name, original) = match &assignment.request.target {
            Target::Existing { session_id } => (session_id.clone(), None),
            Target::Create { request } => {
                (request.name.clone(), Some(request.settings.target.clone()))
            }
        };
        Ok(Choice {
            revision: choice.as_ref().map(|(revision, _)| *revision),
            authorization: assignment.request.authorization,
            name,
            target: choice.map(|(_, choice)| choice.target).or(original),
        })
    }

    async fn model_assignment(&self, operation: &str) -> Result<Assignment, Error> {
        let assignment = self
            .repository
            .read::<Assignment>(&crate::assignment::key(operation)?)
            .await?
            .ok_or(Error::Conflict)?
            .1;
        if assignment.retired {
            return Err(Error::Conflict);
        }
        Ok(assignment)
    }

    pub(super) async fn select_model(
        &self,
        operation: &str,
        expected_revision: Option<u64>,
        target: ExecutionTarget,
        commands: &dyn Commands,
    ) -> Result<View, Error> {
        let assignment = self.model_assignment(operation).await?;
        let ExecutionTarget::Model {
            model,
            thinking_level,
        } = &target
        else {
            return Err(invalid("Select a replacement model"));
        };
        target.validate().map_err(invalid)?;
        let choice = self
            .coordinator
            .models
            .resolve(Selection::Named {
                connection_slug: model.connection_slug.clone(),
                model: model.model.clone(),
            })
            .await
            .map_err(invalid)?
            .ok_or_else(|| invalid("Selected model is unavailable"))?;
        if choice.model != *model
            || thinking_level.is_some_and(|level| !choice.thinking_levels.contains(&level))
        {
            return Err(invalid(
                "Selected model identity or thinking level is unavailable",
            ));
        }
        commands.validate_authority().await?;
        let key = choice_key(&assignment.request.target)?;
        let selected = Selected {
            id: uuid::Uuid::new_v4(),
            target,
        };
        // A choice never rewrites the frozen Route or execution operation.
        self.repository
            .put(&key, expected_revision, &selected)
            .await?;
        let session_id = match &assignment.request.target {
            Target::Existing { session_id } => session_id.clone(),
            Target::Create { request } => self.restore_or_create_root(commands, request).await?,
        };
        let session = commands.session(session_id).await?;
        let current = self
            .repository
            .read::<Selected>(&key)
            .await?
            .ok_or(Error::Conflict)?;
        if current.1.id != selected.id {
            return Err(Error::Contended);
        }
        match commands
            .configure(Configure {
                session_id: session.session_id,
                expected_revision: session.revision,
                target: selected.target,
            })
            .await?
        {
            Configured::Committed { session } => Ok(*session),
            Configured::RevisionConflict { .. } => Err(Error::Contended),
        }
    }

    pub(super) async fn restore_or_create_root(
        &self,
        commands: &dyn Commands,
        request: &CreateRoot,
    ) -> Result<String, Error> {
        if let Some(session) = commands.restore_root(request.operation_id.clone()).await? {
            return Ok(session.session_id);
        }
        let mut proposal = request.clone();
        if let Some((_, choice)) = self
            .repository
            .read::<Selected>(&root_key(&request.operation_id)?)
            .await?
        {
            proposal.settings.target = choice.target;
        }
        match commands.create_root(proposal).await {
            Ok(session) => Ok(session.session_id),
            Err(CommandError::Conflict) => commands
                .restore_root(request.operation_id.clone())
                .await?
                .map(|session| session.session_id)
                .ok_or(Error::Conflict),
            Err(error) => Err(error.into()),
        }
    }
}

fn choice_key(target: &Target) -> Result<String, Error> {
    match target {
        Target::Create { request } => root_key(&request.operation_id),
        Target::Existing { session_id } => {
            Ok(format!("model-choices/session/{}", digest(session_id)?))
        }
    }
}
fn root_key(operation: &str) -> Result<String, Error> {
    Ok(format!("model-choices/root/{}", digest(&operation)?))
}
