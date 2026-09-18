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

use crate::{ConfigError, ConfigurationStore, Result, TransactionMode};
use maka_plugins::{
    credentials::{Record, Write, WriteResult},
    storage::{Namespace, validate_key},
};
use sqlx::SqliteConnection;

impl ConfigurationStore {
    pub async fn plugin_credential(
        &self,
        namespace: Namespace,
        key: String,
    ) -> Result<Option<Record>> {
        validate_key(&key).map_err(invalid)?;
        self.transaction(TransactionMode::Deferred, move |tx| {
            Box::pin(async move { read(tx, &namespace, &key).await })
        })
        .await
    }
    /// Shares the private credential database and its SQL owner, never event-log
    /// history or general plugin storage. A lost response does not undo the CAS.
    pub async fn write_plugin_credential(
        &self,
        namespace: Namespace,
        input: Write,
    ) -> Result<WriteResult> {
        input.validate().map_err(invalid)?;
        self.transaction(TransactionMode::Immediate, move |tx| Box::pin(async move {
            let actual = read(tx, &namespace, &input.key).await?.map(|record| record.revision);
            if actual != input.expected_revision { return Ok(WriteResult::Conflict { actual }); }
            if actual.is_none() {
                let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM plugin_credentials WHERE package_id = ? AND scope_id = ?")
                    .bind(namespace.package()).bind(String::from(namespace.scope().clone())).fetch_one(&mut *tx).await?;
                if count >= 256 { return Err(invalid("plugin credential slot capacity exceeded")); }
            }
            let revision = actual.unwrap_or(0) + 1;
            sqlx::query("INSERT INTO plugin_credentials (package_id, scope_id, key, revision, secret) VALUES (?, ?, ?, ?, ?)
                ON CONFLICT(package_id, scope_id, key) DO UPDATE SET revision = excluded.revision, secret = excluded.secret")
                .bind(namespace.package()).bind(String::from(namespace.scope().clone())).bind(&input.key)
                .bind(revision as i64).bind(input.secret).execute(&mut *tx).await?;
            Ok(WriteResult::Written { revision })
        })).await
    }
}
async fn read(
    tx: &mut SqliteConnection,
    namespace: &Namespace,
    key: &str,
) -> Result<Option<Record>> {
    let row: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT revision, secret FROM plugin_credentials WHERE package_id = ? AND scope_id = ? AND key = ?")
        .bind(namespace.package()).bind(String::from(namespace.scope().clone())).bind(key).fetch_optional(tx).await?;
    row.map(|(revision, secret)| {
        Ok(Record {
            revision: crate::database::unsigned(revision)?,
            secret,
        })
    })
    .transpose()
}
fn invalid(error: impl ToString) -> ConfigError {
    ConfigError::Invalid(error.to_string())
}
