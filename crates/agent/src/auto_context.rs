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

use crate::{Inner, RunError, RunInput, compact, history};
use maka_event_log::context::{LatestMainContext, ModelContextSource};
use maka_runtime::{
    context::{CheckpointMode, ModelRequestContext},
    event::{EventWrite, Fact},
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) fn due(input: &RunInput, source: &ModelContextSource) -> bool {
    let Some(ModelRequestContext {
        declared_window: Some(window),
        ..
    }) = &input.context
    else {
        return false;
    };
    let LatestMainContext::Selected(latest) = &source.latest_main else {
        return false;
    };
    let Some(connection) = input
        .configuration
        .model
        .as_ref()
        .map(|binding| binding.connection_id.as_str())
    else {
        return false;
    };
    if !latest.projection_current
        || latest.model_id != input.provider.model
        || latest.connection_id.as_deref() != Some(connection)
        || latest.checkpoint_event_id.as_deref()
            != source
                .baseline
                .as_ref()
                .map(|baseline| baseline.event_id.as_str())
    {
        return false;
    }
    threshold(
        latest.usage.input_tokens,
        latest.usage.output_tokens,
        *window,
    )
}

fn threshold(input: Option<u64>, output: Option<u64>, window: u64) -> bool {
    let Some(input) = input.filter(|tokens| *tokens > 0) else {
        return false;
    };
    let output = output.unwrap_or(0);
    input
        .saturating_add(output)
        .saturating_add(output.saturating_mul(2).min(8000))
        >= window
}

pub(super) async fn attempt(
    inner: &Arc<Inner>,
    input: &RunInput,
    source: &ModelContextSource,
    mid_turn: bool,
    cancellation: &CancellationToken,
    continuation_base: Option<u64>,
) -> Result<bool, RunError> {
    let opening = source
        .anchor
        .iter()
        .chain(source.tail.iter().filter_map(|event| match event {
            maka_event_log::context::ContextEvent::Canonical(stored) => Some(stored.as_ref()),
            _ => None,
        }))
        .find(|stored| {
            stored.event.invocation.invocation_id == input.invocation.invocation_id
                && matches!(stored.event.fact, Fact::InvocationOpened { .. })
        })
        .ok_or_else(|| {
            RunError::ReconciliationRequired(
                "automatic compaction lacks its canonical opening".into(),
            )
        })?;
    let mode = if mid_turn {
        CheckpointMode::MidTurn {
            anchor_event_id: opening.event.id.clone(),
        }
    } else {
        CheckpointMode::PreTurn
    };
    let result = compact::run(inner, input, &mode, cancellation, continuation_base).await;
    let (_, checkpoint) = match result {
        Ok(result) => result,
        Err(RunError::Model(
            maka_model::ModelError::Adapter(_)
            | maka_model::ModelError::Provider(_)
            | maka_model::ModelError::TimedOut
            | maka_model::ModelError::ContextOverflow { .. },
        )) => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(checkpoint) = checkpoint else {
        return Ok(false);
    };
    // The candidate is a checked text summary plus this exact durable opening.
    // Materialize its attachments with the normal bounded reader before adopting.
    history::materialize(
        &inner.log,
        &[],
        Some(opening),
        &input.invocation.session_id,
        input.supports_vision,
        cancellation,
    )
    .await?;
    if cancellation.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    inner.log.append(&EventWrite::plain(checkpoint)?).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::threshold;
    #[test]
    fn actual_usage_threshold_preserves_unknown_and_caps_reply_reserve() {
        assert!(!threshold(None, Some(5000), 1));
        assert!(!threshold(Some(0), Some(5000), 1));
        assert!(threshold(Some(100), None, 100));
        assert!(!threshold(Some(100), None, 101));
        assert!(threshold(Some(100), Some(20), 160));
        assert!(!threshold(Some(100), Some(20), 161));
        assert!(threshold(Some(100), Some(5000), 13100));
        assert!(!threshold(Some(100), Some(5000), 13101));
    }
}
