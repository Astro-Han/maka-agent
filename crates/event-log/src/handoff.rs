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
use maka_runtime::event::{Fact, InvocationInput, InvocationOutcome, RuntimeEvent};
use sqlx::SqliteConnection;

pub(crate) async fn validate(
    tx: &mut SqliteConnection,
    event: &RuntimeEvent,
) -> Result<(), StoreError> {
    if matches!(event.fact, Fact::InvocationOpened { .. }) {
        let reserved: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_events pause
             WHERE pause.kind='invocation_ended'
             AND json_extract(pause.event_json,'$.invocation.session_id')=?1
             AND json_extract(pause.event_json,'$.fact.outcome.kind')='handoff_paused'
             AND NOT EXISTS(SELECT 1 FROM runtime_events successor WHERE successor.kind='invocation_opened'
               AND json_extract(successor.event_json,'$.fact.input.kind')='handoff'
               AND json_extract(successor.event_json,'$.fact.input.claim.source.invocation.invocation_id')=pause.invocation_id))",
        ).bind(&event.invocation.session_id).fetch_one(&mut *tx).await?;
        if reserved {
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
    let opening: String = sqlx::query_scalar(
        "SELECT event_json FROM runtime_events WHERE invocation_id=? AND kind='invocation_opened'",
    )
    .bind(&event.invocation.invocation_id)
    .fetch_one(&mut *tx)
    .await?;
    let opening: RuntimeEvent = serde_json::from_str(&opening)?;
    if !matches!(
        &opening.fact,
        Fact::InvocationOpened {
            input: InvocationInput::Message { .. } | InvocationInput::Continuation { .. },
            configuration: Some(_),
        }
    ) || pause.intent.root_run_id != event.invocation.run_id
    {
        return Err(invalid(
            "handoff must preserve its admitted logical model Run",
        ));
    }
    // As with pruning before the next step, a failed, effect-free summary is
    // harmless; an interrupted request never proves a provider effect settled.
    crate::context::safety::settled_boundary(tx, &event.invocation.invocation_id, i64::MAX as u64)
        .await?;
    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE kind='invocation_opened'
         AND (invocation_id=?1 OR json_extract(event_json,'$.invocation.run_id')=?2))",
    )
    .bind(&pause.intent.successor_invocation_id)
    .bind(&pause.intent.successor_run_id)
    .fetch_one(&mut *tx)
    .await?;
    if occupied {
        return Err(invalid("handoff successor identity already exists"));
    }
    Ok(())
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
