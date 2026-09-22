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

use crate::{Access, Error, Repository, invalid};
use maka_plugins::{
    authorization::Target as AuthorizationTarget,
    execution::{CommandError, Commands, Configure, Configured, CreateRoot, RootSettings, Target},
    llm::{Models, Selection},
    session::View,
};
use maka_runtime::execution::{CollaborationMode, SandboxMode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const KEY: &str = "coordinator";
#[derive(Serialize, Deserialize)]
struct Intent {
    request: CreateRoot,
}

/// Resolving a coordinator never depends on a reserved Session name or a Host-private path.
pub struct Coordinator {
    pub repository: Arc<Repository>,
    pub access: Arc<Access>,
    pub models: Arc<dyn Models>,
    pub behavior: maka_runtime::execution::BehaviorId,
    pub tools: std::collections::BTreeSet<String>,
}
impl Coordinator {
    pub async fn session_id(&self) -> Result<Option<String>, Error> {
        Ok(self
            .repository
            .read("coordinator-session")
            .await?
            .map(|(_, id)| id))
    }
    pub async fn resolve(&self) -> Result<(View, Arc<dyn Commands>), Error> {
        let target = AuthorizationTarget::PluginWorkspace {
            sandbox_mode: SandboxMode::WorkspaceWrite,
        };
        let commands = self.access.commands(&target).await?;
        self.resolve_with(commands).await
    }

    async fn resolve_with(
        &self,
        commands: Arc<dyn Commands>,
    ) -> Result<(View, Arc<dyn Commands>), Error> {
        let intent = match self.repository.read::<Intent>(KEY).await? {
            Some((_, intent)) => intent,
            None => {
                let model = self
                    .models
                    .resolve(Selection::Default)
                    .await
                    .map_err(invalid)?
                    .ok_or_else(|| invalid("WorkHub default model is unavailable"))?;
                let intent = self.intent(Target::Model {
                    model,
                    thinking_level: None,
                });
                match self.repository.put(KEY, None, &intent).await {
                    Ok(()) => intent,
                    Err(Error::Contended) => {
                        self.repository
                            .read::<Intent>(KEY)
                            .await?
                            .ok_or(Error::Conflict)?
                            .1
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        let operation = intent.request.operation_id.clone();
        let root = match commands.restore_root(operation.clone()).await? {
            Some(root) => root,
            None => match commands.create_root(intent.request).await {
                Ok(root) => root,
                Err(CommandError::Conflict) => commands
                    .restore_root(operation)
                    .await?
                    .ok_or(Error::Conflict)?,
                Err(error) => return Err(error.into()),
            },
        };
        match self
            .repository
            .read::<String>("coordinator-session")
            .await?
        {
            Some((_, id)) if id != root.session_id => return Err(Error::Conflict),
            Some(_) => {}
            None => match self
                .repository
                .put("coordinator-session", None, &root.session_id)
                .await
            {
                Ok(()) => {}
                Err(Error::Contended)
                    if self.session_id().await?.as_deref() == Some(&root.session_id) => {}
                Err(error) => return Err(error),
            },
        }
        let view = commands.session(root.session_id).await?;
        Ok((view, commands))
    }

    /// A user choice can repair an uncreated intent or an already-created coordinator.
    /// The operation identity never changes, even if an older creation wins a race.
    pub async fn select_model(
        &self,
        target: Target,
        commands: Arc<dyn Commands>,
    ) -> Result<View, Error> {
        let Target::Model { model, .. } = &target else {
            return Err(invalid("The coordinator requires a model"));
        };
        target.validate().map_err(invalid)?;
        let resolved = self
            .models
            .resolve(Selection::Named {
                connection_slug: model.connection_slug.clone(),
                model: model.model.clone(),
            })
            .await
            .map_err(invalid)?;
        if resolved.as_ref() != Some(model) {
            return Err(invalid("The selected model is no longer available"));
        }
        // Consent must precede both durable intent and Host work.
        commands.validate_authority().await?;
        let previous = self.repository.read::<Intent>(KEY).await?;
        let intent = self.intent(target.clone());
        self.repository
            .put(KEY, previous.map(|(revision, _)| revision), &intent)
            .await?;
        let (view, commands) = self.resolve_with(commands).await?;
        // Read intent after the Session revision. A competing selection either
        // prevents this CAS or has a newer revision to reconcile against.
        let latest = self
            .repository
            .read::<Intent>(KEY)
            .await?
            .ok_or(Error::Conflict)?
            .1;
        if latest.request.settings.target != target {
            return Err(Error::Contended);
        }
        match commands
            .configure(Configure {
                session_id: view.session_id,
                expected_revision: view.revision,
                target,
            })
            .await?
        {
            Configured::Committed { session } => Ok(*session),
            Configured::RevisionConflict { .. } => Err(Error::Contended),
        }
    }

    fn intent(&self, target: Target) -> Intent {
        Intent {
            request: CreateRoot {
                managed: true,
                operation_id: "coordinator".into(),
                name: "WorkHub".into(),
                settings: RootSettings {
                    target,
                    sandbox_mode: SandboxMode::WorkspaceWrite,
                    approval_policy: maka_runtime::execution::ApprovalPolicy::OnRequest,
                    collaboration_mode: CollaborationMode::Agent,
                    behavior: self.behavior.clone(),
                    bound_tools: Some(self.tools.clone()),
                    instructions: None,
                },
            },
        }
    }
}
