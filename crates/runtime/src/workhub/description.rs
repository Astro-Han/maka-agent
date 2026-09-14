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

use super::DelegationKind;
use crate::execution::{PermissionMode, WorkspaceTarget};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DelegationDescription {
    Existing { name: String },
    Created { name: String, spec: CreateSpec },
}

/// Original user choices, not defaults resolved from later Host configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSpec {
    pub title: String,
    pub workspace: WorkspaceTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defaults: Option<CreateDefaults>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<CreateModel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateModel {
    pub llm_connection_id: String,
    pub llm_connection_slug: String,
    pub model: String,
}

impl DelegationDescription {
    pub fn name(&self) -> &str {
        match self {
            Self::Existing { name } | Self::Created { name, .. } => name,
        }
    }

    pub(super) fn validate(&self, kind: DelegationKind) -> Result<(), &'static str> {
        if self.name().trim().is_empty() || self.name().len() > 4096 {
            return Err("invalid WorkHub target description");
        }
        match (self, kind) {
            (Self::Existing { .. }, DelegationKind::Existing) => Ok(()),
            (Self::Created { spec, .. }, DelegationKind::Created) => {
                if spec.title.trim().is_empty() || spec.title.len() > 4096 {
                    return Err("invalid WorkHub creation title");
                }
                let locator = match &spec.workspace {
                    WorkspaceTarget::Project { project_id } => project_id,
                    WorkspaceTarget::HostPath { path } => path,
                };
                if locator.trim().is_empty() || locator.contains('\0') || locator.len() > 32768 {
                    return Err("invalid WorkHub creation workspace");
                }
                if let Some(model) = spec
                    .defaults
                    .as_ref()
                    .and_then(|defaults| defaults.model.as_ref())
                {
                    for field in [
                        &model.llm_connection_id,
                        &model.llm_connection_slug,
                        &model.model,
                    ] {
                        if field.trim().is_empty() || field.encode_utf16().count() > 512 {
                            return Err("invalid WorkHub creation model");
                        }
                    }
                }
                Ok(())
            }
            _ => Err("WorkHub description does not match its disposition"),
        }
    }
}
