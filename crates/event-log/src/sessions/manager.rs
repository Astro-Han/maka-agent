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

use super::validate_id;
use crate::{EventLog, StoreError};
use maka_plugins::{composition::Scope, storage::Namespace};
use sqlx::Row;

/// Host-bound creation identity, not a caller-provided authorization credential.
#[derive(Clone, Debug)]
pub struct ManagedSession {
    pub session_id: String,
    pub manager: Namespace,
    /// Stable identity of the managed creation request.
    pub fingerprint: String,
}

impl EventLog {
    pub async fn session_manager(&self, session_id: &str) -> Result<Option<Namespace>, StoreError> {
        self.validate_root()?;
        validate_id(session_id)?;
        let session_id = session_id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    sqlx::query(
                        "SELECT package_id, scope_id FROM session_managers WHERE session_id = ?",
                    )
                    .bind(session_id)
                    .fetch_optional(connection)
                    .await?
                    .map(|row| {
                        let scope = Scope::try_from(row.get::<String, _>("scope_id"))
                            .map_err(|error| StoreError::InvalidTransition(error.to_string()))?;
                        Namespace::new(row.get::<String, _>("package_id"), scope)
                            .map_err(|error| StoreError::InvalidTransition(error.to_string()))
                    })
                    .transpose()
                })
            })
            .await
    }
}
