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

#[derive(Clone, Debug, PartialEq)]
pub struct LoginReceipt {
    pub target: Target,
    pub method: String,
    request_fingerprint: String,
    pub connection: ConnectionIdentity,
    pub phase: Phase,
}

impl LoginReceipt {
    pub fn matches(&self, input: &LoginStart) -> bool {
        self.target == input.target
            && self.method == input.authentication.method
            && input
                .fingerprint()
                .is_ok_and(|digest| digest == self.request_fingerprint)
    }

    pub(super) fn new(input: LoginStart, connection: ConnectionIdentity) -> Result<Self> {
        let request_fingerprint = input.fingerprint().map_err(ConfigError::Invalid)?;
        Ok(Self {
            target: input.target,
            method: input.authentication.method,
            request_fingerprint,
            connection,
            phase: Phase::Authenticated,
        })
    }
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
    let saved: Option<(String, String, String, String, String)> =
        sqlx::query_as("SELECT target, method, request_fingerprint, connection, phase FROM oauth_login_receipts WHERE attempt_id = ?")
            .bind(attempt)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((target, method, request_fingerprint, connection, phase)) = saved else {
        return Ok(None);
    };
    let saved = LoginReceipt {
        target: serde_json::from_str(&target)?,
        method,
        request_fingerprint,
        connection: serde_json::from_str(&connection)?,
        phase: serde_json::from_str(&phase)?,
    };
    if !saved.target.matches(&saved.connection)
        || matches!(
            saved.phase,
            Phase::AwaitingAuthorization | Phase::Committing
        )
    {
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
        "INSERT INTO oauth_login_receipts(attempt_id, target, method, request_fingerprint, connection, phase) VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(attempt_id) DO UPDATE SET phase = excluded.phase",
    )
    .bind(attempt)
    .bind(serde_json::to_string(&saved.target)?)
    .bind(&saved.method)
    .bind(&saved.request_fingerprint)
    .bind(serde_json::to_string(&saved.connection)?)
    .bind(serde_json::to_string(&saved.phase)?)
    .execute(&mut *tx)
    .await?;
    // Monotonic SQLite order is internal; no timestamp tie or public counter.
    sqlx::query("DELETE FROM oauth_login_receipts WHERE json_extract(phase, '$.phase') != 'exchanging' AND completion_order NOT IN
        (SELECT completion_order FROM oauth_login_receipts WHERE json_extract(phase, '$.phase') != 'exchanging' ORDER BY completion_order DESC LIMIT 256)")
        .execute(&mut *tx).await?;
    Ok(())
}

pub(super) async fn pending_count(tx: &mut SqliteConnection) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM oauth_login_receipts WHERE json_extract(phase, '$.phase') = 'exchanging'")
        .fetch_one(tx).await?)
}
