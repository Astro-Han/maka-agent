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

use super::*;
use sqlx::SqliteConnection;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginReceipt {
    pub target: Target,
    pub connection: ConnectionIdentity,
}

pub(super) fn validate_attempt(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return Err(ConfigError::Invalid("invalid OAuth attempt ID".into()));
    }
    Ok(())
}

pub(super) async fn read(tx: &mut SqliteConnection, attempt: &str) -> Result<Option<LoginReceipt>> {
    let saved: Option<(String, String)> =
        sqlx::query_as("SELECT target, connection FROM oauth_login_receipts WHERE attempt_id = ?")
            .bind(attempt)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((target, connection)) = saved else {
        return Ok(None);
    };
    let saved = LoginReceipt {
        target: serde_json::from_str(&target)?,
        connection: serde_json::from_str(&connection)?,
    };
    if !saved.target.matches(&saved.connection) {
        return Err(ConfigError::Invalid(
            "inconsistent OAuth receipt identity".into(),
        ));
    }
    Ok(Some(saved))
}

pub(super) async fn write(
    tx: &mut SqliteConnection,
    attempt: &str,
    saved: &LoginReceipt,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO oauth_login_receipts(attempt_id, target, connection) VALUES (?, ?, ?)",
    )
    .bind(attempt)
    .bind(serde_json::to_string(&saved.target)?)
    .bind(serde_json::to_string(&saved.connection)?)
    .execute(&mut *tx)
    .await?;
    // Monotonic SQLite order is internal; no timestamp tie or public counter.
    sqlx::query("DELETE FROM oauth_login_receipts WHERE completion_order NOT IN
        (SELECT completion_order FROM oauth_login_receipts ORDER BY completion_order DESC LIMIT 256)")
        .execute(&mut *tx).await?;
    Ok(())
}
