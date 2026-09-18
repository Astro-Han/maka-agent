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

use super::{MAX_TEXT_BYTES, evidence};
use crate::{EventLog, StoreError};
use maka_presentation::{Content, InvocationView, Message, ProjectionError, watermark};
use maka_runtime::event::Invocation;
use sqlx::Connection;

/// The same failed-fragment verdict drives durable rows and live completion frames.
pub(crate) async fn interrupted_messages(
    connection: &mut sqlx::SqliteConnection,
    invocation: &str,
    step: Option<&str>,
    through: u64,
) -> Result<Vec<String>, StoreError> {
    let through_sql = i64::try_from(through).map_err(|_| ProjectionError::OutOfRange)?;
    let facts = super::evidence::selected(connection, invocation, step, through_sql, true).await?;
    let mut view = InvocationView::new(super::MAX_TEXT_BYTES)?;
    let mut ids = Vec::new();
    for fact in facts {
        let rows = view.push(&fact)?;
        if fact.sequence == through {
            ids.extend(rows.into_iter().filter_map(|row| {
                matches!(
                    row.message.content,
                    Content::Assistant {
                        interrupted: true,
                        ..
                    }
                )
                .then_some(row.message.id)
            }));
        }
    }
    Ok(ids)
}
impl EventLog {
    /// Freeze only the unresolved step at this historical log fence. This read
    /// does not accept its observations as model history or durable transcript.
    pub async fn active_transcript(
        &self,
        invocation: &Invocation,
        through: u64,
    ) -> Result<Vec<Message>, StoreError> {
        watermark(through)?;
        self.validate_root()?;
        let invocation = invocation.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let fence = evidence::fence(&mut tx, through).await?;
                    let id = &invocation.invocation_id;
                    let compact: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM runtime_events
                         WHERE invocation_id = ?1 AND sequence <= ?2
                         AND kind = 'invocation_opened'
                         AND json_extract(event_json, '$.fact.input.kind') = 'context_compact')",
                    )
                    .bind(id)
                    .bind(fence)
                    .fetch_one(&mut *tx)
                    .await?;
                    if compact {
                        return Ok(Vec::new());
                    }
                    let ended: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id = ?1
             AND sequence <= ?2 AND kind = 'invocation_ended')",
                    )
                    .bind(id)
                    .bind(fence)
                    .fetch_one(&mut *tx)
                    .await?;
                    if ended {
                        return Ok(Vec::new());
                    }
                    let step = evidence::pending_step(&mut tx, id, fence).await?;
                    let mut view = InvocationView::new(MAX_TEXT_BYTES)?;
                    for fact in
                        evidence::selected(&mut tx, id, step.as_deref(), fence, false).await?
                    {
                        if fact.event.invocation != invocation {
                            return Err(ProjectionError::Invalid(
                                "active transcript invocation mismatch",
                            )
                            .into());
                        }
                        view.push(&fact)?;
                    }
                    Ok(view.overlay())
                })
            })
            .await
    }
}
