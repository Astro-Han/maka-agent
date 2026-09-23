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
use sqlx::{Connection, SqliteConnection};

/// Shares the accepted work's transaction: a retiring Session cannot accept new
/// work, and an accepted use makes a preparing revision no longer disposable.
pub(crate) async fn retain(
    connection: &mut SqliteConnection,
    session: &str,
) -> Result<(), StoreError> {
    super::removal::require_accepting(connection, session).await?;
    super::copy::retain(connection, session).await
}

impl EventLog {
    /// Native process acceptance has no canonical invocation. The Host pins its
    /// admission gate through this commit and actual launch.
    pub async fn retain_session(&self, session: &str) -> Result<(), StoreError> {
        self.validate_root()?;
        super::validate_id(session)?;
        let session = session.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    retain(&mut tx, &session).await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)
                })
            })
            .await
    }
}
