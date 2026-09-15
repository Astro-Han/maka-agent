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

use crate::StoreError;
use maka_runtime::{
    continuation::{
        ContinuationClaim, MAX_SOURCE_BYTES, MAX_SOURCE_EVENTS, REPLAY_VERSION, ReplayEvidence,
        RunBoundary, SessionBase,
    },
    event::{Fact, InvocationInput, RuntimeEvent},
    handoff::HandoffPause,
};
use sqlx::SqliteConnection;

pub(super) async fn check(
    tx: &mut SqliteConnection,
    seal: &RuntimeEvent,
    opening: &RuntimeEvent,
    pause: &HandoffPause,
) -> Result<(), StoreError> {
    let seal_bytes = envelope_bytes(seal)?;
    crate::run_prefix::read(
        tx,
        &seal.invocation.session_id,
        &seal.invocation.run_id,
        &seal.invocation.invocation_id,
        i64::MAX,
        MAX_SOURCE_EVENTS - 1,
        MAX_SOURCE_BYTES
            .checked_sub(seal_bytes)
            .ok_or(StoreError::PrefixTooLarge)?,
    )
    .await?;
    let Fact::InvocationOpened { configuration, .. } = &opening.fact else {
        return Err(super::invalid(
            "handoff budget requires its canonical opening",
        ));
    };
    // Sizing only: no claim is acquired and no guessed proof is persisted.
    // Reserve the widest ordinals, fixed-width digests and both new envelopes.
    let digest = format!("sha256:{}", "0".repeat(64));
    let successor = RuntimeEvent::new(
        pause.intent.successor(&seal.invocation),
        Fact::InvocationOpened {
            configuration: configuration.clone(),
            input: InvocationInput::Handoff {
                pause: Box::new(pause.clone()),
                claim: Box::new(ContinuationClaim {
                    id: pause.intent.claim_id.clone(),
                    source: RunBoundary {
                        invocation: seal.invocation.clone(),
                        high_water: u64::MAX,
                        digest: digest.clone(),
                    },
                    base: SessionBase {
                        high_water: u64::MAX,
                        digest: digest.clone(),
                    },
                    replay: ReplayEvidence {
                        version: REPLAY_VERSION,
                        digest,
                        route_identity: pause.execution.route_identity.clone(),
                    },
                }),
            },
        },
    );
    let bytes = MAX_SOURCE_BYTES
        .checked_sub(seal_bytes + envelope_bytes(&successor)?)
        .ok_or(StoreError::PrefixTooLarge)?;
    crate::context::read::check_handoff_capacity(tx, opening, MAX_SOURCE_EVENTS - 2, bytes).await
}

fn envelope_bytes(event: &RuntimeEvent) -> Result<usize, StoreError> {
    let mut envelope = event.clone();
    envelope.recorded_at = std::time::UNIX_EPOCH;
    Ok(serde_json::to_vec(&envelope)?.len() + (u64::MAX.ilog10() + u32::MAX.ilog10()) as usize)
}
