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

use crate::{
    StoreError,
    message_resolution::{MessageExecution, owner},
    turns::InvocationState,
};
use maka_runtime::{
    event::{Fact, RuntimeEvent},
    workhub::Delegation,
};
use sqlx::SqliteConnection;

/// One link is unambiguous even when finished. With several, exactly one must
/// still hold work. Bounded pages avoid keeping the entire coordination history.
pub(super) async fn select(
    tx: &mut SqliteConnection,
    session: &str,
) -> Result<(Box<Delegation>, MessageExecution), StoreError> {
    let mut cursor = 0i64;
    let mut count = 0usize;
    let mut first = None;
    let mut working = None;
    let mut working_count = 0usize;
    loop {
        let rows: Vec<(i64, Option<String>)> = sqlx::query_as(
            "SELECT e.sequence, CASE WHEN length(CAST(e.event_json AS BLOB)) <= 1048576 THEN e.event_json END
             FROM runtime_events e WHERE e.kind = 'workhub_delegated' AND e.sequence > ?
             AND json_extract(e.event_json, '$.fact.delegation.target.session_id') = ?
             AND NOT EXISTS(SELECT 1 FROM workhub_stops s
               WHERE s.delegation_action_id = json_extract(e.event_json, '$.fact.delegation.action_id')
               AND s.resolution_json IS NOT NULL
               AND json_extract(s.resolution_json, '$.outcome') != 'not_owned')
             ORDER BY e.sequence LIMIT 32"
        ).bind(cursor).bind(session).fetch_all(&mut *tx).await?;
        if rows.is_empty() {
            break;
        }
        for (sequence, json) in rows {
            cursor = sequence;
            let event: RuntimeEvent =
                serde_json::from_str(&json.ok_or(StoreError::PrefixTooLarge)?)?;
            let Fact::WorkhubDelegated { delegation } = event.fact else {
                return Err(super::invalid("WorkHub delegation index changed"));
            };
            delegation
                .validate(&event.invocation)
                .map_err(super::invalid)?;
            let work = owner::execution(tx, session, &delegation.target_message_id()).await?;
            let retired = match &work {
                MessageExecution::Cancelled => true,
                MessageExecution::Owned(boundary) | MessageExecution::Shared(boundary) => {
                    matches!(boundary.state, InvocationState::Ended { .. })
                }
                MessageExecution::Pending | MessageExecution::Missing => false,
            };
            count += 1;
            if !retired {
                working_count += 1;
                if working.is_none() {
                    working = Some((delegation, work));
                }
            } else if count == 1 {
                first = Some((delegation, work));
            }
        }
    }
    if working_count == 1 {
        return Ok(working.unwrap());
    }
    if count == 1
        && let Some(first) = first
    {
        return Ok(first);
    }
    Err(super::invalid(
        "WorkHub stop requires one unambiguous delegated Message",
    ))
}
