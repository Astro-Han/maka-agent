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

use super::super::{Executions, Result, failure, internal, provider};
use crate::session::SessionConfiguration;
use maka_agent::{RunInput, RunWork};
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::{continuation::RunBoundary, event::Invocation};
use uuid::Uuid;

pub(in crate::execution) enum Mode<'a> {
    Observe(Uuid),
    Prepared(&'a super::super::prepare::Environment),
}
impl Executions {
    pub(in crate::execution) async fn prepare_resume(
        &self,
        session: &SessionConfiguration,
        source: RunBoundary,
        turn_id: String,
        fingerprint: Option<String>,
        mode: Mode<'_>,
    ) -> Result<RunInput> {
        let cwd = std::path::PathBuf::from(&session.workspace.host_cwd);
        let workspace =
            tokio::task::spawn_blocking(move || maka_fs_tools::workspace::read_identity(&cwd))
                .await
                .map_err(internal)?
                .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
        let session_id = &source.invocation.session_id;
        let provider = match &mode {
            Mode::Observe(_) => provider::observe(&self.configuration, session_id, session).await?,
            Mode::Prepared(_) => {
                provider::resolve(&self.configuration, &self.oauth, session_id, session).await?
            }
        };
        let mut configuration = session.observed_configuration(workspace);
        if let Some(source) = self
            .log
            .invocation_configuration(&source.invocation)
            .await
            .map_err(internal)?
        {
            configuration.orchestration_mode = source.orchestration_mode;
        }
        let (tools, system_prompt) = match mode {
            Mode::Prepared(environment) => {
                let super::super::prepare::Backend::Model(model) = &environment.backend else {
                    return Err(super::failure(
                        maka_protocol::OperationErrorCode::OperationUnavailable,
                        "Executor has no native model continuation",
                    ));
                };
                configuration.tool_composition = Some(environment.composition.clone());
                (model.tools.clone(), environment.prompt.clone())
            }
            Mode::Observe(connection) => {
                let system_prompt = session.initial_prompt("").map_err(internal)?;
                let tools = self
                    .preview_tool_catalog(
                        Some(session_id),
                        connection,
                        &configuration.cwd,
                        session.sandbox_mode,
                        session.tool_profile,
                    )
                    .await?;
                (tools, system_prompt)
            }
        };
        configuration.system_prompt = system_prompt;
        Ok(RunInput {
            invocation: Invocation {
                session_id: session_id.clone(),
                turn_id,
                run_id: Uuid::new_v4().to_string(),
                invocation_id: Uuid::new_v4().to_string(),
            },
            work: RunWork::Continuation {
                source,

                tools,
                max_steps: 64,
            },
            request_fingerprint: fingerprint,
            provider: provider.config,
            provider_options: provider.options,
            main_output_limit: provider.main_output_limit,
            supports_vision: provider.supports_vision,
            context: Some(provider.context),
            configuration,
        })
    }
}
