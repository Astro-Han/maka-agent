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

use super::Skills;
use futures_util::future::BoxFuture;
use maka_plugins::contributions::Staged;
use maka_runtime::tools::{PreparationFuture, ToolCallContext, ToolError, ToolPreparer};
use maka_tool_catalog::plugins::{Binding, BindingProvider, BindingRequest, PluginTool};
use serde_json::Value;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) fn publish(skills: &Skills, staged: &mut Staged) -> Result<(), String> {
    let binding: Arc<dyn BindingProvider> = Arc::new(skills.clone());
    for registration in super::snapshot::registrations(Arc::new(Unbound)) {
        let name = registration.definition.name.clone();
        let tool = PluginTool::new(registration)
            .map_err(|error| error.to_string())?
            .with_binding(binding.clone())
            .always_visible();
        staged
            .insert(&name, tool)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

impl BindingProvider for Skills {
    fn bind(
        &self,
        request: BindingRequest,
        workspace: maka_plugins::filesystem::ReadDirectory,
    ) -> BoxFuture<'static, Result<Option<Binding>, ToolError>> {
        let skills = self.clone();
        Box::pin(async move {
            let snapshot = skills
                .capture(&workspace, request.tools.into_iter().collect())
                .await
                .map_err(|error| ToolError::Failed(error.to_string()))?;
            if snapshot.catalog().available().next().is_none() {
                return Ok(None);
            }
            let context = snapshot.catalog().prompt(32 * 1024);
            Ok(Some(Binding {
                handler: Arc::new(snapshot),
                context: (!context.is_empty()).then_some(context),
            }))
        })
    }
}

// Schema staging never provides an executable fallback outside a request binding.
struct Unbound;
impl ToolPreparer for Unbound {
    fn names(&self) -> Vec<String> {
        vec!["Skill".into(), "SkillSearch".into()]
    }
    fn prepare(
        &self,
        _: String,
        _: Value,
        _: ToolCallContext,
        _: CancellationToken,
    ) -> PreparationFuture {
        Box::pin(async { Err(maka_runtime::tool_call::ToolRejection::Unavailable) })
    }
}
