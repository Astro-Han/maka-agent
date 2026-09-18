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

mod execution;
mod packages;
mod storage;

use maka_plugins::{composition::Ledger, package::Package};
use sqlx::{Connection, SqliteConnection};

use crate::{EventLog, StoreError};

pub enum PackageUpdate {
    Install(Package),
    Remove(String),
}

impl EventLog {
    pub async fn plugin_composition(&self) -> Result<Ledger, StoreError> {
        self.validate_root()?;
        self.connection
            .run(|connection| Box::pin(read_ledger(connection)))
            .await
    }

    /// Bytes, installed pointer and desired composition have a single commit boundary.
    /// The caller validates projection/dependencies before publishing this intent.
    pub async fn commit_plugin_state(
        &self,
        mut next: Ledger,
        update: Option<PackageUpdate>,
    ) -> Result<Ledger, StoreError> {
        self.validate_root()?;
        if next.generation >= (1_u64 << 53) - 1 {
            return Err(invalid("composition generation exhausted"));
        }
        next.extend(&[]);
        let expected = next.generation;
        next.generation += 1;
        let encoded = serde_json::to_string(&next)?;
        if encoded.len() > 2 * 1024 * 1024 {
            return Err(invalid("composition intent exceeds 2 MiB"));
        }
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let current = read_ledger(&mut tx).await?;
                    if current.generation != expected {
                        return Err(StoreError::RevisionConflict {
                            expected: expected.to_string(),
                            actual: current.generation.to_string(),
                        });
                    }
                    if let Some(update) = update {
                        match update {
                            PackageUpdate::Install(package) => {
                                packages::install(&mut tx, &package).await?
                            }
                            PackageUpdate::Remove(id) => {
                                let removed =
                                    sqlx::query("DELETE FROM plugin_packages WHERE id = ?")
                                        .bind(id)
                                        .execute(&mut *tx)
                                        .await?;
                                if removed.rows_affected() == 0 {
                                    return Err(invalid("plugin package not found"));
                                }
                            }
                        }
                        // Live instances/clients retain immutable Package bytes in
                        // memory. Historical request evidence does not retain code.
                        sqlx::query("DELETE FROM plugin_package_blobs WHERE digest NOT IN (SELECT digest FROM plugin_packages)")
                            .execute(&mut *tx).await?;
                    }
                    sqlx::query(
                        "UPDATE plugin_composition SET ledger_json = ? WHERE singleton = 1",
                    )
                    .bind(encoded)
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_modify(|_| {});
                    Ok(next)
                })
            })
            .await
    }

    pub async fn plugin_package(&self, id: &str) -> Result<Option<Package>, StoreError> {
        self.validate_root()?;
        let id = id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let digest: Option<String> =
                        sqlx::query_scalar("SELECT digest FROM plugin_packages WHERE id = ?")
                            .bind(&id)
                            .fetch_optional(&mut *connection)
                            .await?;
                    let Some(digest) = digest else {
                        return Ok(None);
                    };
                    let package = packages::read(connection, &digest).await?;
                    if package.manifest().id != id {
                        return Err(invalid(
                            "installed package identity differs from its manifest",
                        ));
                    }
                    Ok(Some(package))
                })
            })
            .await
    }

    /// Small metadata query; package BLOBs are loaded only when needed.
    pub async fn plugin_packages(&self) -> Result<Vec<(String, String)>, StoreError> {
        self.validate_root()?;
        self.connection
            .run(|connection| {
                Box::pin(async move {
                    Ok(
                        sqlx::query_as("SELECT id, digest FROM plugin_packages ORDER BY id")
                            .fetch_all(connection)
                            .await?,
                    )
                })
            })
            .await
    }
}

async fn read_ledger(connection: &mut SqliteConnection) -> Result<Ledger, StoreError> {
    let encoded: Option<String> = sqlx::query_scalar(
        "SELECT CASE WHEN length(CAST(ledger_json AS BLOB)) <= 2097152 THEN ledger_json END
         FROM plugin_composition WHERE singleton = 1",
    )
    .fetch_one(connection)
    .await?;
    Ok(serde_json::from_str(&encoded.ok_or_else(|| {
        invalid("composition exceeds byte limit")
    })?)?)
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidTransition(message.into())
}
