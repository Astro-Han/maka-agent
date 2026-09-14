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

use crate::{Inner, RunError, RunInput, model_attempt};
use maka_model::prompt::{AssistantPart, Message};
use maka_runtime::{
    context::ModelPurpose,
    continuation::{ContinuationClaim, REPLAY_VERSION, ReplayEvidence, RunBoundary, SessionBase},
    event::Fact,
    input::InvocationInput,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Derive admission evidence from the exact sealed source. The caller supplies
/// only its boundary, never a replay digest or a pre-authorized claim.
pub(super) async fn prepare(
    inner: &Arc<Inner>,
    input: &RunInput,
    source: &RunBoundary,
    catalog: &maka_tools::ToolCatalog,
    cancellation: &CancellationToken,
) -> Result<ContinuationClaim, RunError> {
    let prefix = inner
        .log
        .run_prefix(
            &source.invocation.session_id,
            &source.invocation.run_id,
            None,
            10_000,
            8 * 1024 * 1024,
        )
        .await?
        .ok_or_else(|| invalid("continuation source is missing"))?;
    if prefix.invocation != source.invocation
        || prefix.high_water != source.high_water
        || prefix.digest != source.digest
        || !matches!(
            prefix.events.last().map(|s| &s.event.fact),
            Some(Fact::InvocationEnded { .. })
        )
    {
        return Err(invalid(
            "continuation source is not its exact sealed boundary",
        ));
    }
    let Fact::InvocationOpened {
        input: opening,
        configuration: Some(configuration),
    } = &prefix
        .events
        .first()
        .ok_or_else(|| invalid("continuation source is empty"))?
        .event
        .fact
    else {
        return Err(invalid("continuation source has no observed configuration"));
    };
    if input.configuration.workspace_identity.is_none()
        || input.configuration.workspace_identity != configuration.workspace_identity
    {
        return Err(invalid("continuation workspace identity changed"));
    }
    let base = match opening {
        InvocationInput::Message { .. } => {
            let base = inner
                .log
                .context_before_run(&source.invocation, 10_000, 8 * 1024 * 1024)
                .await?;
            SessionBase {
                high_water: base.source_evidence.high_water,
                digest: base.source_evidence.digest,
            }
        }
        InvocationInput::Continuation { claim, .. } => claim.base.clone(),
        _ => return Err(invalid("source is not a resumable model Run")),
    };
    let context = inner
        .log
        .read_lineage_context(source, 10_000, 8 * 1024 * 1024)
        .await?;
    let prompt = model_attempt::prompt(
        inner,
        input,
        &context,
        ModelPurpose::Main,
        cancellation,
        Some(base.high_water),
    )
    .await?;
    if !matches!(
        prompt.iter().find(|m| !matches!(m, Message::System { .. })),
        Some(Message::User { .. })
    ) || !matches!(
        prompt.last(),
        Some(Message::User { .. } | Message::Tool { .. })
    ) && !matches!(prompt.last(), Some(Message::Assistant { content, .. }) if matches!(content.last(), Some(AssistantPart::ToolResult { .. })))
    {
        return Err(invalid(
            "continuation requires stable user/tool replay boundaries",
        ));
    }
    let tools = maka_tools::RunTools::new(
        inner.log.clone(),
        input.invocation.clone(),
        catalog.clone(),
        input.configuration.tool_mode,
        inner.cells.clone(),
    );
    let definitions = tools.capture().definitions();
    let mut available: std::collections::HashSet<_> = catalog.names().into_iter().collect();
    available.extend(definitions.iter().map(|definition| definition.name.clone()));
    for message in &prompt {
        if let Message::Assistant { content, .. } = message {
            for part in content {
                if let AssistantPart::ToolCall {
                    tool_name,
                    provider_executed,
                    ..
                } = part
                    && *provider_executed != Some(true)
                    && !available.contains(tool_name)
                {
                    return Err(invalid("continuation requires an unavailable tool"));
                }
            }
        }
    }
    let request = model_attempt::prepare_request(input, prompt, definitions, ModelPurpose::Main)?;
    let claim = ContinuationClaim {
        id: uuid::Uuid::new_v4().to_string(),
        source: source.clone(),
        base,
        replay: ReplayEvidence {
            version: REPLAY_VERSION,
            digest: request.input_digest,
            route_identity: request.route_identity,
        },
    };
    claim.validate(&input.invocation).map_err(invalid)?;
    if cancellation.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    Ok(claim)
}

fn invalid(reason: &str) -> RunError {
    RunError::ReconciliationRequired(reason.into())
}
