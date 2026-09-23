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
use sqlx::Connection;
use uuid::Uuid;

impl EventLog {
    /// The native owner holds Host admission through this commit and spawn.
    /// Incomplete records survive restart: a lost handle is not proof of exit.
    pub async fn admit_session_process(&self, session: &str, id: Uuid) -> Result<(), StoreError> {
        self.validate_root()?;
        super::validate_id(session)?;
        let session = session.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    super::retain(&mut tx, &session).await?;
                    sqlx::query("INSERT INTO session_processes(id,session_id) VALUES (?,?)")
                        .bind(id.to_string())
                        .bind(session)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)
                })
            })
            .await
    }

    /// Call only after the owned process group/Job and native resources drained.
    pub async fn clean_session_process(&self, id: Uuid) -> Result<(), StoreError> {
        self.validate_root()?;
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let changed = sqlx::query("UPDATE session_processes SET cleaned=1 WHERE id=?")
                        .bind(id.to_string())
                        .execute(&mut *tx)
                        .await?
                        .rows_affected();
                    if changed != 1 {
                        return Err(StoreError::EventConflict);
                    }
                    tx.commit().await.map_err(StoreError::CommitUnknown)
                })
            })
            .await
    }
}
