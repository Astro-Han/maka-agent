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

use super::invalid;
use crate::{EventLog, StoreError};
use maka_plugins::storage::{Data, Mutation, Namespace, Record, validate_key};
use sqlx::{Connection, SqliteConnection};
use std::collections::BTreeSet;

impl EventLog {
    pub async fn plugin_data(
        &self,
        namespace: &Namespace,
        key: &str,
    ) -> Result<Option<Record>, StoreError> {
        self.validate_root()?;
        validate_key(key).map_err(|error| invalid(&error.to_string()))?;
        let namespace = namespace.clone();
        let key = key.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move { read(connection, &namespace, &key).await })
            })
            .await
    }

    /// All compares and writes commit atomically within one package/scope.
    pub async fn plugin_data_batch(
        &self,
        namespace: &Namespace,
        mutations: Vec<Mutation>,
    ) -> Result<Vec<Record>, StoreError> {
        self.validate_root()?;
        if mutations.is_empty() || mutations.len() > 128 {
            return Err(invalid("plugin data batch must contain 1..=128 mutations"));
        }
        let mut keys = BTreeSet::new();
        let mut encoded = Vec::new();
        let mut total = 0;
        for mutation in &mutations {
            mutation
                .validate()
                .map_err(|error| invalid(&error.to_string()))?;
            if !keys.insert(&mutation.key) {
                return Err(invalid("duplicate plugin data batch key"));
            }
            let value = mutation
                .data
                .value()
                .map(serde_json::to_string)
                .transpose()?;
            total += value.as_ref().map_or(0, String::len);
            if total > 8 * 1024 * 1024 {
                return Err(invalid("plugin data batch exceeds 8 MiB"));
            }
            encoded.push(value);
        }
        let namespace = namespace.clone();
        let commits = self.commits.clone();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let mut records = Vec::new();
            for (mutation, encoded) in mutations.into_iter().zip(encoded) {
                let current = read(&mut tx, &namespace, &mutation.key).await?;
                let actual = current.as_ref().map(|record| record.revision);
                if actual != mutation.expected_revision {
                    return Err(StoreError::RevisionConflict {
                        expected: mutation.expected_revision.map_or_else(|| "absent".into(), |revision| revision.to_string()),
                        actual: actual.map_or_else(|| "absent".into(), |revision| revision.to_string()),
                    });
                }
                let revision = actual.unwrap_or(0) + 1;
                if revision > (1 << 53) - 1 { return Err(invalid("plugin data revision exhausted")); }
                sqlx::query("INSERT INTO plugin_data (package_id, scope_id, key, revision, value_json) VALUES (?, ?, ?, ?, ?)
                    ON CONFLICT(package_id, scope_id, key) DO UPDATE SET revision = excluded.revision, value_json = excluded.value_json")
                    .bind(namespace.package()).bind(String::from(namespace.scope().clone())).bind(&mutation.key)
                    .bind(revision as i64).bind(encoded).execute(&mut *tx).await?;
                records.push(Record { revision, data: mutation.data });
            }
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            commits.send_modify(|_| {});
            Ok(records)
        })).await
    }
}

async fn read(
    connection: &mut SqliteConnection,
    namespace: &Namespace,
    key: &str,
) -> Result<Option<Record>, StoreError> {
    let row: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT revision, value_json FROM plugin_data WHERE package_id = ? AND scope_id = ? AND key = ?")
        .bind(namespace.package()).bind(String::from(namespace.scope().clone())).bind(key)
        .fetch_optional(connection).await?;
    row.map(|(revision, value)| {
        let revision =
            u64::try_from(revision).map_err(|_| invalid("invalid stored plugin data revision"))?;
        if revision == 0 || revision > (1 << 53) - 1 {
            return Err(invalid("invalid stored plugin data revision"));
        }
        Ok(Record {
            revision,
            data: match value {
                Some(value) => Data::Present(serde_json::from_str(&value)?),
                None => Data::Deleted,
            },
        })
    })
    .transpose()
}
