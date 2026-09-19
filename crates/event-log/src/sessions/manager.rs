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
use sqlx::{Connection, Row};

/// A trusted Host declaration, not a caller-provided authorization credential.
#[derive(Clone, Debug)]
pub struct ManagedSession {
    pub session_id: String,
    pub manager: Namespace,
    /// Only this create receipt may materialize the reserved identity.
    pub fingerprint: String,
}

impl EventLog {
    /// Reserve before admitting clients. This does not adopt or rewrite existing
    /// data: the manager must still prove its exact creation receipt before use.
    /// A malformed existing Session must not open an ordinary execution bypass.
    pub async fn reserve_managed_session(&self, claim: &ManagedSession) -> Result<(), StoreError> {
        self.validate_root()?;
        validate_id(&claim.session_id)?;
        if claim.manager.scope() == &Scope::DesktopUi {
            return Err(StoreError::InvalidTransition(
                "Desktop UI cannot manage Host Sessions".into(),
            ));
        }
        if claim.fingerprint.is_empty() || claim.fingerprint.len() > 512 {
            return Err(StoreError::InvalidTransition(
                "invalid session fingerprint length".into(),
            ));
        }
        let claim = claim.clone();
        let scope = String::from(claim.manager.scope().clone());
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            if let Some(row) = sqlx::query("SELECT package_id, scope_id, fingerprint FROM session_managers WHERE session_id = ?")
                .bind(&claim.session_id).fetch_optional(&mut *tx).await? {
                if row.get::<String, _>("package_id") != claim.manager.package()
                    || row.get::<String, _>("scope_id") != scope
                    || row.get::<String, _>("fingerprint") != claim.fingerprint {
                    return Err(StoreError::SessionConflict);
                }
                return Ok(());
            }
            sqlx::query("INSERT INTO session_managers VALUES (?, ?, ?, ?)")
                .bind(claim.session_id).bind(claim.manager.package()).bind(scope)
                .bind(claim.fingerprint).execute(&mut *tx).await?;
            tx.commit().await.map_err(StoreError::CommitUnknown)
        })).await
    }

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
