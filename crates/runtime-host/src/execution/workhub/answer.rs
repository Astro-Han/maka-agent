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

use super::{
    super::{Executions, Result, failure, internal, provider},
    profile,
};
use crate::{
    plugins::workhub::answer::{Plan, Request},
    session::SessionConfiguration,
};
use maka_agent::{RunInput, RunWork};
use maka_event_log::sessions::SessionRecord;
use maka_plugins::fiber::Context;
use maka_protocol::{OperationErrorCode as Code, workhub::TurnResult};
use maka_runtime::{event::Invocation, workhub::COORDINATION_SESSION_ID};
use std::sync::Arc;
use uuid::Uuid;

impl Executions {
    /// A canonical opening is authoritative even after plugin retirement or
    /// model removal. Reading it never admits a new request.
    pub(crate) async fn workhub_answer_receipt(
        &self,
        request: &Request,
    ) -> Result<Option<TurnResult>> {
        let Some(record) = self
            .recorded(COORDINATION_SESSION_ID, &request.input.turn_id)
            .await?
        else {
            return Ok(None);
        };
        if record.fingerprint.as_deref() != Some(&request.fingerprint) {
            return Err(failure(
                Code::OperationConflict,
                "WorkHub Turn belongs to another request",
            ));
        }
        Ok(Some(TurnResult {
            turn_id: request.input.turn_id.clone(),
        }))
    }
}

pub(super) async fn execute(
    executions: &Arc<Executions>,
    caller: Context,
    plan: Plan,
    connection: Uuid,
    root_id: &str,
) -> Result<TurnResult> {
    let session = {
        let _gate = executions.lock_admission().await;
        if let Some(receipt) = executions.workhub_answer_receipt(&plan.request).await? {
            return Ok(receipt);
        }
        let _call = caller
            .admit()
            .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
        current(executions, &plan).await?
    };
    // No shared admission lock around credentials or workspace initialization.
    // Preserve errors until the receipt recheck: a concurrent identical request
    // may already have committed while preparation was in flight.
    let prepared = async {
        let provider = provider::observe(
            &executions.configuration,
            COORDINATION_SESSION_ID,
            &session.configuration,
        )
        .await?;
        let configuration = session
            .configuration
            .invocation_configuration()
            .await
            .map_err(internal)?;
        Ok::<_, maka_protocol::OperationError>((provider, configuration))
    }
    .await;
    let _gate = executions.lock_admission().await;
    if let Some(receipt) = executions.workhub_answer_receipt(&plan.request).await? {
        return Ok(receipt);
    }
    let _call = caller
        .admit()
        .map_err(|error| failure(Code::OperationUnavailable, &error.to_string()))?;
    current(executions, &plan).await?;
    let (provider, mut configuration) = prepared?;
    let content = plan.request.input.content();
    executions
        .validate_message_content(COORDINATION_SESSION_ID, &content, root_id)
        .await?;
    let (tools, composition) =
        profile::tools(executions, &session.configuration, connection, &plan.policy)?;
    let provider = provider.admit(&executions.oauth)?;
    configuration.tool_mode = plan.tool_mode;
    configuration.system_prompt = Some(plan.policy.prompt.clone());
    configuration.tool_composition = Some(composition);
    executions
        .launch(RunInput {
            invocation: Invocation {
                session_id: COORDINATION_SESSION_ID.into(),
                turn_id: plan.request.input.turn_id.clone(),
                run_id: Uuid::new_v4().to_string(),
                invocation_id: Uuid::new_v4().to_string(),
            },
            work: RunWork::Message {
                source_messages: Vec::new(),
                skill_invocation: None,
                message: content.into(),
                tools,
                max_steps: plan.max_steps,
            },
            request_fingerprint: Some(plan.request.fingerprint),
            provider: provider.config,
            provider_options: provider.options,
            main_output_limit: provider.main_output_limit,
            supports_vision: provider.supports_vision,
            context: Some(provider.context),
            configuration,
        })
        .await?;
    Ok(TurnResult {
        turn_id: plan.request.input.turn_id,
    })
}

async fn current(
    executions: &Executions,
    plan: &Plan,
) -> Result<SessionRecord<SessionConfiguration>> {
    if executions.shutdown.is_cancelled() {
        return Err(failure(Code::HostDraining, "Host is draining"));
    }
    let session = executions
        .workhub_coordinator()
        .await?
        .ok_or_else(|| failure(Code::NotFound, "WorkHub Session has not been resolved"))?;
    if session.configuration_digest != plan.configuration_digest {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub configuration changed during preparation",
        ));
    }
    if executions
        .has_session_work(COORDINATION_SESSION_ID)
        .await
        .map_err(internal)?
    {
        return Err(failure(
            Code::SessionBusy,
            "WorkHub has active or pending work",
        ));
    }
    Ok(session)
}
