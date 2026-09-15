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

use crate::{EventLog, StoreError};
use maka_runtime::event::{Fact, InvocationInput, InvocationOutcome, RuntimeEvent};
use sqlx::{Connection, SqliteConnection};

mod budget;
mod cancel;
mod owner;
pub(crate) use cancel::apply as cancel;
pub(crate) use owner::read as owner;

impl EventLog {
    /// Ordinary queued work cannot cross an unclaimed cooperative seal.
    pub async fn has_pending_handoff(&self, session: &str) -> Result<bool, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session)?;
        let session = session.to_owned();
        self.connection
            .run(move |tx| Box::pin(async move { Ok(reservation(tx, &session).await?.is_some()) }))
            .await
    }

    /// Eligibility only. The source still owns execution until its seal commits.
    pub async fn check_handoff(
        &self,
        invocation: &maka_runtime::event::Invocation,
        pause: &maka_runtime::handoff::HandoffPause,
    ) -> Result<(), StoreError> {
        self.validate_root()?;
        let event = RuntimeEvent::new(
            invocation.clone(),
            Fact::InvocationEnded {
                outcome: InvocationOutcome::HandoffPaused {
                    pause: pause.clone(),
                },
            },
        );
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    validate(&mut tx, &event).await?;
                    tx.commit().await?;
                    Ok(())
                })
            })
            .await
    }
}

pub(crate) async fn validate(
    tx: &mut SqliteConnection,
    event: &RuntimeEvent,
) -> Result<(), StoreError> {
    if let Fact::InvocationOpened {
        input,
        configuration,
    } = &event.fact
    {
        let source = match input {
            InvocationInput::Handoff { claim, .. } => {
                Some(claim.source.invocation.invocation_id.as_str())
            }
            _ => None,
        };
        if reservation_conflict(
            tx,
            &event.invocation.run_id,
            &event.invocation.invocation_id,
            input.inherited_claim().map(|claim| claim.id.as_str()),
            source,
        )
        .await?
        {
            return Err(invalid("opening would steal a reserved handoff identity"));
        }
        // Admission permits one physical owner per Session. A paused owner
        // blocks the next opening, so only the latest opening can reserve it.
        let reserved = reservation(tx, &event.invocation.session_id).await?;
        if let InvocationInput::Handoff { claim, pause } = input {
            if reserved.as_deref() != Some(&claim.source.invocation.invocation_id) {
                return Err(invalid(
                    "handoff does not own the pending Session reservation",
                ));
            }
            let source = &claim.source.invocation;
            let (opening, terminal): (String, String) = sqlx::query_as(
                "SELECT o.event_json, t.event_json FROM runtime_events o JOIN runtime_events t
                 ON t.invocation_id=o.invocation_id AND t.kind='invocation_ended'
                 WHERE o.invocation_id=? AND o.kind='invocation_opened'",
            )
            .bind(&source.invocation_id)
            .fetch_one(&mut *tx)
            .await?;
            let opening: RuntimeEvent = serde_json::from_str(&opening)?;
            let terminal: RuntimeEvent = serde_json::from_str(&terminal)?;
            if !matches!(terminal.fact, Fact::InvocationEnded { outcome: InvocationOutcome::HandoffPaused { pause: sealed } } if sealed == **pause)
                || !matches!(opening.fact, Fact::InvocationOpened { configuration: Some(ref frozen), .. } if Some(frozen) == configuration.as_ref())
            {
                return Err(invalid(
                    "handoff changes its sealed intent, budget or admitted configuration",
                ));
            }
        } else if reserved.is_some() {
            return Err(invalid(
                "Session is reserved for its sealed handoff successor",
            ));
        }
        return Ok(());
    }
    let Fact::InvocationEnded {
        outcome: InvocationOutcome::HandoffPaused { pause },
    } = &event.fact
    else {
        return Ok(());
    };
    pause.validate(&event.invocation).map_err(invalid)?;
    let ended: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id=? AND kind='invocation_ended')",
    ).bind(&event.invocation.invocation_id).fetch_one(&mut *tx).await?;
    if ended {
        return Err(invalid("handoff source is already sealed"));
    }
    let opening: String = sqlx::query_scalar(
        "SELECT event_json FROM runtime_events WHERE invocation_id=? AND kind='invocation_opened'",
    )
    .bind(&event.invocation.invocation_id)
    .fetch_one(&mut *tx)
    .await?;
    let opening: RuntimeEvent = serde_json::from_str(&opening)?;
    if opening.invocation != event.invocation {
        return Err(invalid("handoff source invocation changed"));
    }
    let Fact::InvocationOpened {
        input,
        configuration: Some(configuration),
    } = &opening.fact
    else {
        return Err(invalid("handoff has no admitted configuration"));
    };
    let workspace = configuration
        .workspace_identity
        .as_ref()
        .ok_or_else(|| invalid("handoff requires an observed workspace identity"))?;
    crate::context::safety::require_safe(
        tx,
        &event.invocation.session_id,
        Some(&event.invocation.invocation_id),
    )
    .await?;
    if let Some(claim) = input.inherited_claim()
        && crate::continuation::ancestors(tx, &claim.source, &claim.base, workspace)
            .await?
            .len()
            >= maka_runtime::continuation::MAX_ANCESTRY
    {
        return Err(invalid(
            "handoff would exceed the continuation lineage capacity",
        ));
    }
    let root = match &opening.fact {
        Fact::InvocationOpened {
            input: InvocationInput::Message { .. } | InvocationInput::Continuation { .. },
            configuration: Some(_),
        } => &event.invocation.run_id,
        Fact::InvocationOpened {
            input: InvocationInput::Handoff {
                pause: previous, ..
            },
            configuration: Some(_),
        } if pause.remaining_steps <= previous.remaining_steps => &previous.intent.root_run_id,
        _ => {
            return Err(invalid(
                "handoff requires an admitted model Run with remaining budget",
            ));
        }
    };
    let replay_base = match input {
        InvocationInput::Continuation { claim, .. } => Some(claim.base.high_water),
        InvocationInput::Handoff { pause, .. } => pause.execution.replay_base,
        _ => None,
    };
    if pause.execution.replay_base != replay_base {
        return Err(invalid("handoff changes its admitted history projection"));
    }
    if &pause.intent.root_run_id != root {
        return Err(invalid(
            "handoff must preserve its admitted logical model Run",
        ));
    }
    // As with pruning before the next step, a failed, effect-free summary is
    // harmless; an interrupted request never proves a provider effect settled.
    crate::context::safety::settled_boundary(tx, &event.invocation.invocation_id, i64::MAX as u64)
        .await?;
    crate::interactions::lifecycle::require_closed(tx, &event.invocation).await?;
    budget::check(tx, event, &opening, pause).await?;
    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE kind='invocation_opened'
         AND (invocation_id=?1 OR json_extract(event_json,'$.invocation.run_id')=?2
           OR json_extract(event_json,'$.fact.input.claim.id')=?3))",
    )
    .bind(&pause.intent.successor_invocation_id)
    .bind(&pause.intent.successor_run_id)
    .bind(&pause.intent.claim_id)
    .fetch_one(&mut *tx)
    .await?;
    if occupied {
        return Err(invalid("handoff successor identity already exists"));
    }
    if reservation_conflict(
        tx,
        &pause.intent.successor_run_id,
        &pause.intent.successor_invocation_id,
        Some(&pause.intent.claim_id),
        None,
    )
    .await?
    {
        return Err(invalid("handoff successor identity is already reserved"));
    }
    Ok(())
}

async fn reservation(
    tx: &mut SqliteConnection,
    session: &str,
) -> Result<Option<String>, StoreError> {
    Ok(sqlx::query_scalar(
        "SELECT pause.invocation_id FROM runtime_events pause
         WHERE pause.kind='invocation_ended' AND json_extract(pause.event_json,'$.fact.outcome.kind')='handoff_paused'
         AND pause.invocation_id=(SELECT invocation_id FROM runtime_events WHERE kind='invocation_opened'
           AND json_extract(event_json,'$.invocation.session_id')=? ORDER BY sequence DESC LIMIT 1)"
    ).bind(session).fetch_optional(tx).await?)
}

async fn reservation_conflict(
    tx: &mut SqliteConnection,
    run: &str,
    invocation: &str,
    claim: Option<&str>,
    source: Option<&str>,
) -> Result<bool, StoreError> {
    Ok(sqlx::query_scalar(
        "WITH reservations AS NOT MATERIALIZED (SELECT event_json FROM runtime_events
         WHERE kind='invocation_ended' AND json_extract(event_json,'$.fact.outcome.kind')='handoff_paused'
         AND (?4 IS NULL OR invocation_id != ?4))
         SELECT EXISTS(SELECT 1 FROM reservations WHERE json_extract(event_json,'$.fact.outcome.pause.intent.successor_run_id')=?1
           UNION ALL SELECT 1 FROM reservations WHERE json_extract(event_json,'$.fact.outcome.pause.intent.successor_invocation_id')=?2
           UNION ALL SELECT 1 FROM reservations WHERE json_extract(event_json,'$.fact.outcome.pause.intent.claim_id')=?3)",
    ).bind(run).bind(invocation).bind(claim).bind(source).fetch_one(tx).await?)
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
