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
    /// One read snapshot, bounded resident records, no independent candidate authority.
    /// The Host supplies configuration eligibility; durable execution safety stays here.
    pub async fn workhub_candidates<T, F>(
        &self,
        eligible: F,
    ) -> Result<Vec<SessionRecord<T>>, StoreError>
    where
        T: DeserializeOwned + Send + 'static,
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
                            if !eligible(&record)
                                || record.execution.as_ref().is_some_and(|execution| {
                                    matches!(execution.state, SessionExecutionState::Live { .. })
                                })
                            {
                                continue;
                            }
                            let pending: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM message_admissions WHERE session_id = ?)"
                    ).bind(id).fetch_one(&mut *tx).await?;
                            if pending || crate::shell_runs::unsettled(&mut tx, id).await? {
                                continue;
                            }
                            match crate::context::safety::require_safe(&mut tx, id, None).await {
                                Ok(()) => {}
                                Err(StoreError::InvalidTransition(_)) => continue,
                                Err(error) => return Err(error),
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
