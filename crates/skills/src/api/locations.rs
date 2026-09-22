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

use crate::{SkillScope, SkillSource};
use serde::{Deserialize, Serialize};

/// The same ordered locations feed discovery and directory management.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LocationId {
    #[serde(rename = "project:maka")]
    ProjectMaka,
    #[serde(rename = "project:agents")]
    ProjectAgents,
    #[serde(rename = "workspace:legacy")]
    Workspace,
    #[serde(rename = "user:maka")]
    UserMaka,
    #[serde(rename = "user:agents")]
    UserAgents,
}
impl LocationId {
    pub const ALL: [Self; 5] = [
        Self::ProjectMaka,
        Self::ProjectAgents,
        Self::Workspace,
        Self::UserMaka,
        Self::UserAgents,
    ];
    pub fn reference(self) -> &'static str {
        match self {
            Self::ProjectMaka => "project:maka",
            Self::ProjectAgents => "project:agents",
            Self::Workspace => "workspace:legacy",
            Self::UserMaka => "user:maka",
            Self::UserAgents => "user:agents",
        }
    }
    pub fn directory(self) -> &'static str {
        match self {
            Self::ProjectMaka | Self::UserMaka => ".maka/skills",
            Self::ProjectAgents | Self::UserAgents => ".agents/skills",
            Self::Workspace => "skills",
        }
    }
    pub fn scope(self) -> SkillScope {
        match self {
            Self::ProjectMaka | Self::ProjectAgents => SkillScope::Project,
            Self::UserMaka | Self::UserAgents => SkillScope::User,
            Self::Workspace => SkillScope::Workspace,
        }
    }
    pub fn source(self) -> SkillSource {
        match self {
            Self::ProjectMaka | Self::UserMaka => SkillSource::Maka,
            Self::ProjectAgents | Self::UserAgents => SkillSource::Agents,
            Self::Workspace => SkillSource::Legacy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationStatus {
    Available,
    Missing,
    BlockedPath,
    ReadFailed,
    Unavailable,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub id: LocationId,
    pub path: Option<String>,
    pub status: LocationStatus,
    pub valid_count: usize,
    pub invalid_count: usize,
}
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LocationAction {
    List,
    Open {
        id: LocationId,
        expected_path: String,
        create_if_missing: bool,
    },
}
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum LocationResult {
    Locations { locations: Vec<Location> },
    Resolved { path: String },
    Rejected { reason: LocationRejection },
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationRejection {
    Changed,
    Missing,
    BlockedPath,
    ReadFailed,
    Unavailable,
}
