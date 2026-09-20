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

use crate::{EventLog, StoreError, turns::TurnBoundary};
use maka_runtime::{
    continuation::{ContinuationClaim, MAX_SOURCE_BYTES, MAX_SOURCE_EVENTS, RunBoundary},
    event::{EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome, RuntimeEvent},
};
use sqlx::{Connection, SqliteConnection};

impl EventLog {
    /// Settle an unclaimed seal without loading a provider or replaying effects.
    /// If another actor already claimed it, return the current physical owner;
    /// stopping that live execution remains the caller's responsibility.
    pub async fn cancel_handoff(&self, source: &Invocation) -> Result<TurnBoundary, StoreError> {
        self.validate_root()?;
        for id in [
            &source.session_id,
            &source.turn_id,
            &source.run_id,
            &source.invocation_id,
        ] {
            crate::sessions::validate_id(id)?;
        }
        let source = source.clone();
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let (boundary, sequence) = apply(&mut tx, &source).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    if let Some(sequence) = sequence {
                        commits.send_replace(sequence);
                    }
                    Ok(boundary)
                })
            })
            .await
    }
}

pub(crate) async fn apply(
    tx: &mut SqliteConnection,
    source: &Invocation,
) -> Result<(TurnBoundary, Option<u64>), StoreError> {
    let prefix = crate::run_prefix::read(
        tx,
        &source.session_id,
        &source.run_id,
        &source.invocation_id,
        i64::MAX,
        MAX_SOURCE_EVENTS,
        MAX_SOURCE_BYTES,
    )
    .await?;
    if prefix.invocation != *source {
        return Err(super::invalid("handoff cancellation source changed"));
    }
    let Some(seal) = prefix.events.last() else {
        return Err(super::invalid("handoff cancellation source is missing"));
    };
    let Fact::InvocationEnded {
        outcome: InvocationOutcome::HandoffPaused { pause },
    } = &seal.event.fact
    else {
        return Err(super::invalid(
            "handoff cancellation requires a sealed pause",
        ));
    };
    let claimed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id=? AND kind='invocation_opened')"
            ).bind(&pause.intent.successor_invocation_id).fetch_one(&mut *tx).await?;
    if claimed {
        return Ok((super::owner(tx, source).await?, None));
    }
    let opening = prefix.events.first().expect("checked nonempty prefix");
    let Fact::InvocationOpened {
        input,
        configuration: Some(configuration),
    } = &opening.event.fact
    else {
        return Err(super::invalid(
            "handoff source has no admitted configuration",
        ));
    };
    let base = match input {
        InvocationInput::Message { .. } => {
            crate::context::frozen::fresh_base(tx, source, opening.sequence).await?
        }
        InvocationInput::Handoff { claim, .. } | InvocationInput::Continuation { claim, .. } => {
            claim.base.clone()
        }
        _ => return Err(super::invalid("handoff source is not a model Run")),
    };
    let invocation = pause.intent.successor(source);
    let opening = RuntimeEvent::new(
        invocation.clone(),
        Fact::InvocationOpened {
            configuration: Some(configuration.clone()),
            input: InvocationInput::Handoff {
                pause: Box::new(pause.clone()),
                claim: Box::new(ContinuationClaim {
                    id: pause.intent.claim_id.clone(),
                    source: RunBoundary {
                        invocation: source.clone(),
                        high_water: prefix.high_water,
                        digest: prefix.digest,
                    },
                    base,
                    replay: pause.execution.replay.clone(),
                }),
            },
        },
    );
    let writes = [
        EventWrite::plain(opening.clone()),
        EventWrite::plain(RuntimeEvent::new(
            invocation,
            Fact::InvocationEnded {
                outcome: InvocationOutcome::Cancelled {
                    source: "runtime_cancellation".into(),
                },
            },
        )),
    ]
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| super::invalid(&e.to_string()))?;
    let mut last = 0;
    for write in &writes {
        last = match EventLog::append_in_transaction(tx, write).await? {
            crate::append::AppendResult::Existing(sequence)
            | crate::append::AppendResult::Inserted(sequence) => sequence,
        };
    }
    crate::context::validate_batch(tx, &writes).await?;
    let boundary = crate::turns::project(tx, opening).await?;
    Ok((boundary, Some(last)))
}
