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

use crate::{EventLog, StoreError, append::AppendResult};
use maka_runtime::{
    event::{EventWrite, Fact},
    workhub::DelegationKind,
};
use serde::Serialize;
use sqlx::Connection;

impl EventLog {
    /// Creation, delegation and pending delivery commit together. Replays never
    /// recreate a Session or replace its subsequently edited configuration.
    pub async fn create_workhub_session<T: Serialize>(
        &self,
        action: &EventWrite,
        configuration: &T,
    ) -> Result<u64, StoreError> {
        self.validate_root()?;
        let Fact::WorkhubDelegated { delegation } = &action.event().fact else {
            return Err(super::invalid("WorkHub creation requires a delegation"));
        };
        delegation
            .validate(&action.event().invocation)
            .map_err(super::invalid)?;
        if delegation.kind != DelegationKind::Created {
            return Err(super::invalid("WorkHub creation requires a new Session"));
        }
        let configuration = serde_json::to_string(configuration)?;
        let action = action.clone();
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE event_id = ?)",
                    )
                    .bind(&action.event().id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if !exists {
                        let Fact::WorkhubDelegated { delegation } = &action.event().fact else {
                            unreachable!()
                        };
                        let now = action
                            .event()
                            .recorded_at
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_err(|_| super::invalid("invalid WorkHub creation time"))?
                            .as_millis()
                            .try_into()
                            .map_err(|_| super::invalid("invalid WorkHub creation time"))?;
                        crate::sessions::insert(
                            &mut tx,
                            &delegation.target.session_id,
                            &format!("workhub.create:{}", delegation.request_fingerprint),
                            &configuration,
                            now,
                        )
                        .await?;
                    }
                    match Self::append_in_transaction(&mut tx, &action).await? {
                        AppendResult::Existing(sequence) => {
                            tx.rollback().await?;
                            Ok(sequence)
                        }
                        AppendResult::Inserted(sequence) => {
                            tx.commit().await.map_err(StoreError::CommitUnknown)?;
                            commits.send_replace(sequence);
                            Ok(sequence)
                        }
                    }
                })
            })
            .await
    }
}
