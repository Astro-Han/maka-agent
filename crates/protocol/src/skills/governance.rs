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

use super::{invalid, text};
use crate::Result;
pub use maka_runtime::skills::{SkillScope, SkillSource, SkillValidationCode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceSourceType {
    Workspace,
    Bundled,
    Managed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Ok,
    MissingLock,
    Modified,
    MetadataError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedUpdateStatus {
    NotManaged,
    SourceMissing,
    UpToDate,
    UpdateAvailable,
    LocalModified,
    MetadataError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillRuntimeStatus {
    Enabled,
    Disabled,
    StateError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextStatus {
    Unknown,
    Advertised,
    Disabled,
    Invalid,
    HostIncompatible,
    Shadowed,
    Budget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GovernanceItem {
    #[serde(rename = "ref")]
    pub reference: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub declared_tools: Vec<String>,
    pub metadata_truncated: bool,
    pub source_type: GovernanceSourceType,
    pub user_modified: bool,
    pub validation_status: ValidationStatus,
    pub validation_codes: Vec<SkillValidationCode>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub managed_update_status: Option<ManagedUpdateStatus>,
    pub enabled: bool,
    pub pinned: bool,
    pub runtime_status: SkillRuntimeStatus,
    pub scope: SkillScope,
    pub source: SkillSource,
    pub context_status: ContextStatus,
    #[serde(deserialize_with = "Option::deserialize")]
    pub context_rank: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub shadowed_by: Option<String>,
    pub needs_review: bool,
    pub manageable: bool,
}
impl GovernanceItem {
    pub(super) fn validate(&self) -> Result<()> {
        identity(&self.reference, 512)?;
        identity(&self.id, 256)?;
        if let Some(reference) = &self.shadowed_by {
            identity(reference, 512)?;
        }
        if self.name.len() > 256
            || self.description.len() > 4096
            || self.declared_tools.len() > 64
            || self.validation_codes.len() > 64
            || self
                .context_rank
                .is_some_and(|rank| !(1..=9_007_199_254_740_991).contains(&rank))
        {
            return Err(invalid("Invalid Skill governance projection"));
        }
        for tool in &self.declared_tools {
            text(tool, 256)?;
        }
        Ok(())
    }
}
fn identity(value: &str, limit: usize) -> Result<()> {
    text(value, limit)?;
    if value.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}') {
        return Err(invalid("Invalid Skill identity"));
    }
    Ok(())
}
