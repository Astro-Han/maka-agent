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
    EventLog, StoreError,
    sessions::{SessionExecutionState, SessionRecord},
};
use serde::de::DeserializeOwned;
use sqlx::Connection;

impl EventLog {
    /// Revalidate an offered target independently of the current display window.
    pub async fn workhub_candidate<T, F>(
        &self,
        id: &str,
        eligible: F,
    ) -> Result<Option<SessionRecord<T>>, StoreError>
    where
        T: DeserializeOwned + Send + Sync + 'static,
        F: Fn(&SessionRecord<T>) -> bool + Send + 'static,
    {
        self.validate_root()?;
        crate::sessions::validate_id(id)?;
        let id = id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let Some(record) = crate::sessions::read(&mut tx, &id).await? else {
                        return Ok(None);
                    };
                    if !eligible(&record) || !idle(&mut tx, &record).await? {
                        return Ok(None);
                    }
                    Ok(Some(record))
                })
            })
            .await
    }

    /// One read snapshot, bounded resident records, no independent candidate authority.
    /// The Host supplies configuration eligibility; durable execution safety stays here.
    pub async fn workhub_candidates<T, F>(
        &self,
        eligible: F,
    ) -> Result<Vec<SessionRecord<T>>, StoreError>
    where
        T: DeserializeOwned + Send + Sync + 'static,
        F: Fn(&SessionRecord<T>) -> bool + Send + 'static,
    {
        self.validate_root()?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let mut cursor = String::new();
                    let mut candidates = Vec::new();
                    loop {
                        let ids: Vec<String> = sqlx::query_scalar(
                            "SELECT id FROM session_control WHERE archived = 0 AND id > ?
                     ORDER BY id LIMIT 32",
                        )
                        .bind(&cursor)
                        .fetch_all(&mut *tx)
                        .await?;
                        if ids.is_empty() {
                            break;
                        }
                        for id in &ids {
                            let record = crate::sessions::read(&mut tx, id)
                                .await?
                                .ok_or(StoreError::SessionNotFound)?;
                            if !eligible(&record) || !idle(&mut tx, &record).await? {
                                continue;
                            }
                            candidates.push(record);
                            candidates.sort_by(|a, b| {
                                activity_at(b)
                                    .cmp(&activity_at(a))
                                    .then_with(|| a.id.cmp(&b.id))
                            });
                            candidates.truncate(32);
                        }
                        cursor = ids.last().unwrap().clone();
                    }
                    tx.commit().await?;
                    Ok(candidates)
                })
            })
            .await
    }
}

async fn idle<T>(
    tx: &mut sqlx::SqliteConnection,
    record: &SessionRecord<T>,
) -> Result<bool, StoreError> {
    if record.archived
        || record
            .execution
            .as_ref()
            .is_some_and(|execution| matches!(execution.state, SessionExecutionState::Live { .. }))
    {
        return Ok(false);
    }
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_admissions WHERE session_id = ?)")
            .bind(&record.id)
            .fetch_one(&mut *tx)
            .await?;
    if pending || crate::shell_runs::unsettled(tx, &record.id).await? {
        return Ok(false);
    }
    match crate::context::safety::require_safe(tx, &record.id, None).await {
        Ok(()) => Ok(true),
        Err(StoreError::InvalidTransition(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn activity_at<T>(record: &SessionRecord<T>) -> u64 {
    record
        .execution
        .as_ref()
        .map_or(record.created_at, |execution| {
            execution.last_message.as_ref().map_or_else(
                || match execution.state {
                    SessionExecutionState::Live { recorded_at }
                    | SessionExecutionState::Ended { recorded_at, .. } => recorded_at,
                },
                |message| message.recorded_at,
            )
        })
}
