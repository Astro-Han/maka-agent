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

use super::{Executions, Result, failure, internal, provider};
use maka_agent::{RunInput, RunWork};
use maka_client_capability::RestoredBindings;
use maka_event_log::PendingHandoff;
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::{
    continuation::{MAX_SOURCE_BYTES, MAX_SOURCE_EVENTS, RunBoundary},
    event::{Fact, InvocationOutcome},
    execution::InvocationConfiguration,
    handoff::HandoffPause,
};
use std::{path::Path, sync::Arc};

pub(super) struct PreparedHandoff {
    pub source: RunBoundary,
    pub pause: HandoffPause,
    configuration: InvocationConfiguration,
    bindings: RestoredBindings,
    tools: maka_tools::ToolCatalog,
    directory: maka_fs_tools::workspace::directory::PublishedDirectory,
}

impl Executions {
    pub(super) async fn prepare_handoff(
        &self,
        pending: &PendingHandoff,
    ) -> Result<PreparedHandoff> {
        let source = &pending.invocation;
        let prefix = self
            .log
            .run_prefix(
                &source.session_id,
                &source.run_id,
                None,
                MAX_SOURCE_EVENTS,
                MAX_SOURCE_BYTES,
            )
            .await
            .map_err(internal)?
            .ok_or_else(|| unavailable("Handoff source is unavailable"))?;
        let Fact::InvocationEnded {
            outcome: InvocationOutcome::HandoffPaused { pause },
        } = &prefix
            .events
            .last()
            .ok_or_else(|| unavailable("Handoff source is empty"))?
            .event
            .fact
        else {
            return Err(unavailable("Handoff source is not sealed"));
        };
        if prefix.invocation != *source || pause.intent.host_epoch != pending.host_epoch {
            return Err(unavailable("Handoff identity changed"));
        }
        let pause = pause.clone();
        let Fact::InvocationOpened {
            configuration: Some(configuration),
            ..
        } = &prefix.events.first().expect("nonempty source").event.fact
        else {
            return Err(unavailable("Handoff has no admitted configuration"));
        };
        let configuration = configuration.clone();
        let boundary = RunBoundary {
            invocation: prefix.invocation,
            high_water: prefix.high_water,
            digest: prefix.digest,
        };
        drop(prefix.events);
        let proof = configuration
            .tool_composition
            .as_ref()
            .ok_or_else(|| unavailable("Handoff has no admitted Host composition"))?;
        let record = self
            .log
            .get_session::<crate::session::SessionConfiguration>(&source.session_id)
            .await
            .map_err(internal)?
            .filter(|record| !record.archived)
            .ok_or_else(|| unavailable("Handoff Session is unavailable"))?;
        let (bindings, snapshot) = self
            .capabilities
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .restore_bindings(&source.session_id, &proof.clients)
            .map_err(|error| unavailable(&error.to_string()))?;
        let mut additional = maka_tools::ClientTools::new(
            snapshot,
            self.capabilities.registry.clone(),
            self.capabilities.broker.clone(),
            configuration.cwd.clone(),
            self.interactions.clone(),
        )
        .registrations();
        for name in &proof.private_clients {
            self.plugin_catalog
                .reserve::<maka_tools::plugins::PluginTool>(name)
                .map_err(internal)?;
        }
        additional.retain(|tool| !proof.private_clients.contains(&tool.definition.name));
        additional.push(self.interactions.question_tool());
        let mut native = self.native_tools(&configuration.cwd, record.configuration.tool_profile);
        native.set = proof.native_tools;
        let mode = configuration.permission_mode;
        let ceiling = proof.bound_tools.clone();
        let tools = tokio::task::spawn_blocking(move || {
            super::super::tools::catalog(native, mode, additional, ceiling.as_ref())
        })
        .await
        .map_err(internal)??;
        if tools.digest() != pause.execution.tools.catalog_digest {
            return Err(unavailable("Handoff tool catalog changed"));
        }
        let tools = tools
            .with_plugins(
                self.plugin_catalog.clone(),
                maka_plugins::composition::Scope::Session(source.session_id.clone()),
                proof.bound_tools.clone(),
            )
            .map_err(internal)?;
        let cwd = configuration.cwd.clone();
        let expected = configuration.workspace_identity.clone();
        let directory = tokio::task::spawn_blocking(move || {
            let identity = maka_fs_tools::workspace::read_identity(Path::new(&cwd))
                .map_err(|error| unavailable(&error.to_string()))?;
            if expected.as_ref() != Some(&identity) {
                return Err(unavailable("Handoff workspace identity changed"));
            }
            maka_fs_tools::workspace::directory::PublishedDirectory::open(Path::new(&cwd))
                .map_err(|error| unavailable(&error.to_string()))
        })
        .await
        .map_err(internal)??;
        Ok(PreparedHandoff {
            source: boundary,
            pause,
            configuration: *configuration,
            bindings,
            tools,
            directory,
        })
    }
}

impl PreparedHandoff {
    /// Caller has rechecked the canonical owner under execution admission.
    pub(super) async fn start(self, executions: &Arc<Executions>) -> Result<bool> {
        self.directory
            .validate(Path::new(&self.configuration.cwd))
            .map_err(|error| unavailable(&error.to_string()))?;
        let target = self
            .configuration
            .model
            .as_ref()
            .ok_or_else(|| unavailable("Handoff has no admitted model"))?;
        let observed = provider::observe_binding(
            &executions.configuration,
            &self.source.invocation.session_id,
            target,
            self.configuration.thinking_level,
        )
        .await?;
        let run = RunInput {
            invocation: self.pause.intent.successor(&self.source.invocation),
            request_fingerprint: None,
            provider: observed.config.clone(),
            provider_options: self.pause.execution.provider_options.clone(),
            main_output_limit: self.pause.execution.main_output_limit,
            supports_vision: self.pause.execution.supports_vision,
            context: self.pause.execution.context.clone(),
            configuration: self.configuration,
            work: RunWork::Handoff {
                source: self.source,
                pause: Box::new(self.pause),
                tools: self.tools,
            },
        };
        executions
            .engine
            .check_continuation(&run, &executions.shutdown)
            .await
            .map_err(super::super::execution_error)?;
        if !executions
            .capabilities
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .commit_restored_bindings(self.bindings)
            .map_err(|error| unavailable(&error.to_string()))?
        {
            return Ok(false);
        }
        observed.admit(&executions.oauth)?;
        let running = executions
            .engine
            .start(run, executions.shutdown.child_token())
            .await
            .map_err(|error| {
                if super::super::requires_drain(&error) {
                    executions.begin_drain();
                }
                super::super::execution_error(error)
            })?;
        executions.track(running);
        Ok(true)
    }
}

fn unavailable(message: &str) -> maka_protocol::OperationError {
    failure(Code::OperationUnavailable, message)
}
