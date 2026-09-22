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
    execution::{Commands, CreateRoot, RootSettings, Target},
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
        let intent = match self.repository.read::<Intent>(KEY).await? {
            Some((_, intent)) => intent,
            None => {
                let model = self
                    .models
                    .resolve(Selection::Default)
                    .await
                    .map_err(invalid)?
                    .ok_or_else(|| invalid("WorkHub default model is unavailable"))?;
                let intent = Intent {
                    request: CreateRoot {
                        managed: true,
                        operation_id: "coordinator".into(),
                        name: "WorkHub".into(),
                        settings: RootSettings {
                            target: Target::Model {
                                model,
                                thinking_level: None,
                            },
                            sandbox_mode: SandboxMode::WorkspaceWrite,
                            approval_policy: maka_runtime::execution::ApprovalPolicy::OnRequest,
                            collaboration_mode: CollaborationMode::Agent,
                            behavior: self.behavior.clone(),
                            bound_tools: Some(self.tools.clone()),
                            instructions: None,
                        },
                    },
                };
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
        let root = commands.create_root(intent.request).await?;
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
}
