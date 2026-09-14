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

use crate::{EventLog, StoreError, sequence_number};
use futures_util::TryStreamExt;
use maka_runtime::event::{Fact, Invocation, RuntimeEvent, StoredEvent};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row, SqliteConnection};

/// One immutable physical Run segment, not a mutable Session history or a replay plan.
/// `high_water` counts Run-local events (one-based), while StoredEvent.sequence
/// remains the root ledger position. Neither number can stand in for the other.
#[derive(Debug)]
pub struct RunPrefix {
    pub invocation: Invocation,
    pub high_water: u64,
    pub digest: String,
    pub events: Vec<StoredEvent>,
}

impl EventLog {
    /// Resolve identity, exact Run-local cut, budgets and bytes in one SQL snapshot.
    /// A requested cut must exist; it is never silently clamped to the latest event.
    /// An unsealed prefix is readable evidence, not proof that resuming it is safe.
    pub async fn run_prefix(
        &self,
        session: &str,
        run: &str,
        up_to: Option<u64>,
        max_events: usize,
        max_bytes: usize,
    ) -> Result<Option<RunPrefix>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session)?;
        crate::sessions::validate_id(run)?;
        if up_to.is_some_and(|n| n == 0 || n > 9_007_199_254_740_991) {
            return Err(invalid("Run high-water is not a positive safe integer"));
        }
        let (session, run) = (session.to_owned(), run.to_owned());
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let Some(invocation) =
                        crate::turns::run_invocation(&mut tx, &session, &run).await?
                    else {
                        return Ok(None);
                    };
                    let through = if let Some(up_to) = up_to {
                        sqlx::query_scalar::<_, i64>(
                            "SELECT sequence FROM runtime_events WHERE invocation_id = ?
                     ORDER BY sequence LIMIT 1 OFFSET ?",
                        )
                        .bind(&invocation)
                        .bind((up_to - 1) as i64)
                        .fetch_optional(&mut *tx)
                        .await?
                        .ok_or_else(|| invalid("requested Run prefix is unavailable"))?
                    } else {
                        sqlx::query_scalar::<_, i64>(
                            "SELECT MAX(sequence) FROM runtime_events WHERE invocation_id = ?",
                        )
                        .bind(&invocation)
                        .fetch_one(&mut *tx)
                        .await?
                    };
                    let prefix = read(
                        &mut tx,
                        &session,
                        &run,
                        &invocation,
                        through,
                        max_events,
                        max_bytes,
                    )
                    .await?;
                    tx.commit().await?;
                    Ok(Some(prefix))
                })
            })
            .await
    }
}

pub(crate) async fn read(
    connection: &mut SqliteConnection,
    session: &str,
    run: &str,
    invocation_id: &str,
    through: i64,
    max_events: usize,
    max_bytes: usize,
) -> Result<RunPrefix, StoreError> {
    let (count, bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(length(CAST(event_json AS BLOB))), 0)
         FROM runtime_events WHERE invocation_id = ? AND sequence <= ?",
    )
    .bind(invocation_id)
    .bind(through)
    .fetch_one(&mut *connection)
    .await?;
    let high_water = sequence_number(count)?;
    if high_water > max_events as u64 || sequence_number(bytes)? > max_bytes as u64 {
        return Err(StoreError::PrefixTooLarge);
    }
    let mut invocation = None;
    let mut events = Vec::new();
    let mut digest = Sha256::new();
    digest.update(b"maka.run-prefix.v1\0");
    digest.update(high_water.to_be_bytes());
    let mut rows = sqlx::query(
        "SELECT sequence, event_json,
         (SELECT length(payload) FROM tool_result_payloads WHERE event_id = runtime_events.event_id)
         FROM runtime_events WHERE invocation_id = ? AND sequence <= ? ORDER BY sequence",
    )
    .bind(invocation_id)
    .bind(through)
    .fetch(&mut *connection);
    while let Some(row) = rows.try_next().await? {
        let sequence = sequence_number(row.try_get(0)?)?;
        let json: &str = row.try_get(1)?;
        let event: RuntimeEvent = serde_json::from_str(json)?;
        crate::tool_payloads::verify_binding(&event, row.try_get(2)?)?;
        match &invocation {
            None => {
                if !matches!(event.fact, Fact::InvocationOpened { .. })
                    || event.invocation.session_id != session
                    || event.invocation.run_id != run
                    || event.invocation.invocation_id != invocation_id
                {
                    return Err(invalid("Run prefix lacks its exact canonical opening"));
                }
                let identity = serde_json::to_vec(&event.invocation)?;
                digest.update((identity.len() as u64).to_be_bytes());
                digest.update(identity);
                invocation = Some(event.invocation.clone());
            }
            Some(expected)
                if *expected != event.invocation
                    || matches!(event.fact, Fact::InvocationOpened { .. }) =>
            {
                return Err(invalid("Run prefix identity changed"));
            }
            Some(_) => {}
        }
        digest.update(sequence.to_be_bytes());
        digest.update((json.len() as u64).to_be_bytes());
        digest.update(json.as_bytes());
        events.push(StoredEvent { sequence, event });
    }
    Ok(RunPrefix {
        invocation: invocation.ok_or_else(|| invalid("Run prefix is empty"))?,
        high_water,
        digest: format!("sha256:{:x}", digest.finalize()),
        events,
    })
}

fn invalid(reason: &str) -> StoreError {
    StoreError::InvalidTransition(reason.into())
}
