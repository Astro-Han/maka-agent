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

use super::{Result, source_error};
use crate::{
    server::Host,
    session::{PreparedSession, SessionConfiguration},
};
use maka_event_log::bundle::StagedBundle;
use maka_protocol::session::*;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Only user-facing metadata crosses the boundary. Source paths, credentials,
/// executor/behavior choices, tool ceilings and execution policy are not grants.
#[derive(Deserialize)]
struct Metadata {
    name: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    is_flagged: bool,
    #[serde(default)]
    title_is_manual: bool,
}

pub(super) async fn resolve(
    host: &Host,
    staged: &mut StagedBundle,
    workspace: WorkspaceTarget,
) -> Result<BTreeMap<String, SessionConfiguration>> {
    let destination = super::super::workspace::resolve(host, &workspace).await?;
    let mut configurations = BTreeMap::new();
    let mut defaults: Option<SessionConfiguration> = None;
    let ids: Vec<_> = staged
        .summary()
        .inventory
        .sessions
        .iter()
        .map(|s| s.id.clone())
        .collect();
    for id in ids {
        let metadata: Metadata = staged.configuration(&id).await.map_err(source_error)?;
        let input = SessionCreateInput {
            session_id: id.clone(),
            workspace: workspace.clone(),
            target: SessionCreateTarget::Model {
                model_target: SessionModelTarget::Default,
            },
            name: Some(metadata.name),
            labels: Some(
                metadata
                    .labels
                    .into_iter()
                    .filter(|label| label != "mode:bot")
                    .collect(),
            ),
            mode: None,
            thinking_level: SessionThinkingPreference::ModelDefault,
            tool_profile: None,
            sandbox_mode: None,
            approval_policy: None,
            collaboration_mode: None,
            orchestration_mode: None,
        };
        let input = decode_session_create_input(&serde_json::to_value(input).map_err(|e| {
            super::failure(
                maka_protocol::OperationErrorCode::InvalidRequest,
                &e.to_string(),
            )
        })?)
        .map_err(super::super::invalid)?;
        let prepared = PreparedSession::new(input).map_err(super::super::invalid)?;
        let mut config = if let Some(defaults) = &defaults {
            let mut config = prepared.bind(
                destination.clone(),
                defaults.target.clone(),
                defaults.sandbox_mode,
            );
            config.thinking_level = defaults.thinking_level;
            config
        } else {
            let config = super::super::create::resolve(
                &host.configuration,
                prepared,
                SessionThinkingPreference::ModelDefault,
                destination.clone(),
            )
            .await?;
            defaults = Some(config.clone());
            config
        };
        config.is_flagged = metadata.is_flagged;
        config.title_is_manual = metadata.title_is_manual;
        host.executions.validate_workspace(&config)?;
        configurations.insert(id, config);
    }
    Ok(configurations)
}
