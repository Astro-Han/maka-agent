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

use super::{control, current, invalid};
use crate::{EventLog, StoreError};
use maka_graph::control::Wake;
use sqlx::Connection;

impl EventLog {
    /// The signal identity selects its first frozen message. Later projection
    /// changes do not retarget a wake that may already have been accepted by Host.
    pub async fn commit_graph_wake(&self, wake: Wake) -> Result<Wake, StoreError> {
        self.validate_root()?;
        wake.validate().map_err(invalid)?;
        let encoded = serde_json::to_string(&wake)?;
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let existing: Option<String> = sqlx::query_scalar(
                "SELECT request_json FROM graph_wakes WHERE graph_id = ? AND snapshot_key = ?"
            ).bind(wake.graph_id.as_str()).bind(&wake.snapshot_key).fetch_optional(&mut *tx).await?;
                    if let Some(existing) = existing {
                        let previous: Wake = serde_json::from_str(&existing)?;
                        previous.validate().map_err(invalid)?;
                        if previous.request.operation_id != wake.request.operation_id
                            || previous.request.session_id != wake.request.session_id
                        {
                            return Err(StoreError::EventConflict);
                        }
                        return Ok(previous);
                    }
                    let state = control(&mut tx, &wake.graph_id)
                        .await?
                        .ok_or_else(|| invalid("graph not found"))?;
                    if state.closed() {
                        return Err(invalid("graph is closed"));
                    }
                    if wake.request.session_id != state.epoch.root_session_id
                        || current(&mut tx, &state.epoch.root_session_id)
                            .await?
                            .as_ref()
                            != Some(&wake.graph_id)
                    {
                        return Err(StoreError::EventConflict);
                    }
                    let archived: bool =
                        sqlx::query_scalar("SELECT archived FROM session_control WHERE id = ?")
                            .bind(&state.epoch.root_session_id)
                            .fetch_one(&mut *tx)
                            .await?;
                    if archived {
                        return Err(invalid("graph Session is archived"));
                    }
                    sqlx::query("INSERT INTO graph_wakes VALUES (?, ?, ?)")
                        .bind(wake.graph_id.as_str())
                        .bind(&wake.snapshot_key)
                        .bind(encoded)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_modify(|_| {});
                    Ok(wake)
                })
            })
            .await
    }
}
