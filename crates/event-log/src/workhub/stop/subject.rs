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
use maka_runtime::workhub::ActionId;
use maka_runtime::workhub::Delegation;
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
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT e.sequence, json_extract(e.event_json, '$.fact.delegation.action_id')
             FROM workhub_assignments e WHERE e.sequence > ?
             AND json_extract(e.event_json, '$.fact.delegation.target.session_id') = ?
             AND NOT EXISTS(SELECT 1 FROM workhub_corrections c
               WHERE c.replaces_action_id = json_extract(e.event_json, '$.fact.delegation.action_id')
               AND c.resolution_kind IS NOT NULL)
             AND NOT EXISTS(SELECT 1 FROM workhub_stops s
               WHERE s.delegation_action_id = json_extract(e.event_json, '$.fact.delegation.action_id')
               AND s.resolution_json IS NOT NULL
               AND json_extract(s.resolution_json, '$.outcome') != 'not_owned')
             ORDER BY e.sequence LIMIT 32"
        ).bind(cursor).bind(session).fetch_all(&mut *tx).await?;
        if rows.is_empty() {
            break;
        }
        for (sequence, action) in rows {
            let action = ActionId::new(action).map_err(super::invalid)?;
            cursor = sequence;
            let delegation = super::super::assignment::read(tx, &action)
                .await?
                .ok_or_else(|| super::invalid("WorkHub delegation index changed"))?
                .delegation;
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
