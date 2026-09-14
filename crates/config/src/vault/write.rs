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

use super::{status, status_basis};
use crate::{ConfigError, Result, catalog};
use maka_runtime::configuration::{CredentialLocator, CredentialState};
use sqlx::SqliteConnection;
use uuid::Uuid;

pub(crate) async fn write_secret(
    tx: &mut SqliteConnection,
    locator: &CredentialLocator,
    secret: &str,
    now: u64,
) -> Result<()> {
    let previous = status(tx, locator).await?;
    let actual = status_basis(&previous);
    let (credential_id, previous_revision) = match previous.state {
        CredentialState::Absent => (Uuid::new_v4().to_string(), 0),
        CredentialState::Configured {
            credential_id,
            revision,
            ..
        } => (credential_id, revision),
    };
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM credentials")
        .fetch_one(&mut *tx)
        .await?;
    if actual.is_none() && count >= 2048 {
        return Err(ConfigError::Invalid(
            "credential vault exceeds 2048 entries".into(),
        ));
    }
    let revision = catalog::next_revision(previous_revision)?;
    let locator = serde_json::to_string(locator)?;
    sqlx::query("INSERT INTO credentials VALUES(?, ?, ?, ?, ?)
        ON CONFLICT(locator) DO UPDATE SET revision = excluded.revision, secret = excluded.secret, updated_at = excluded.updated_at")
        .bind(locator).bind(credential_id).bind(revision as i64).bind(secret).bind(now as i64)
        .execute(&mut *tx).await?;
    let bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(length(CAST(secret AS BLOB)) + length(locator) + 128), 0) FROM credentials",
    ).fetch_one(&mut *tx).await?;
    let bytes = crate::database::unsigned(bytes)?;
    if bytes > 2 * 1024 * 1024 {
        return Err(ConfigError::Invalid(
            "credential vault exceeds 2 MiB".into(),
        ));
    }
    Ok(())
}
