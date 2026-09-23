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

use super::{BundleError, BundleSummary, format::Record, reader::Reader};
use crate::StoreError;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tokio::io::AsyncRead;

static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./src/bundle/migrations");

/// Private decoded transfer, not yet accepted history. Only Host-authored SQL
/// opens this staging database; input never supplies a SQLite file or schema.
/// Dropping it also closes SQLite's automatically deleted temporary database.
pub struct StagedBundle {
    database: SqliteConnection,
    summary: BundleSummary,
}

impl StagedBundle {
    pub async fn read<R: AsyncRead + Unpin>(input: R) -> Result<Self, BundleError> {
        // An empty SQLite filename creates a private, disk-backed temporary
        // database whose lifetime belongs to the connection, including Drop.
        let mut database = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename("")
                .create_if_missing(true)
                .pragma("temp_store", "FILE")
                .pragma("page_size", "4096")
                .pragma("cache_size", "-4096")
                .pragma("max_page_count", "393216"),
        )
        .await
        .map_err(StoreError::from)?;
        let result = async {
            MIGRATIONS.run_direct(None, &mut database, false).await?;
            let mut reader = Reader::new(input).await?;
            let mut tx = database.begin().await?;
            let mut number = 0i64;
            loop {
                let record = reader.next().await?;
                if matches!(record, Record::End { .. }) {
                    break;
                }
                number += 1;
                sqlx::query("INSERT INTO frames(number,record_json) VALUES(?,?)")
                    .bind(number)
                    .bind(serde_json::to_string(&record)?)
                    .execute(&mut *tx)
                    .await?;
                if matches!(record, Record::Blob(_)) {
                    let mut offset = 0i64;
                    while let Some(bytes) = reader.blob_chunk().await? {
                        let size = bytes.len() as i64;
                        sqlx::query("INSERT INTO chunks(frame,offset,payload) VALUES(?,?,?)")
                            .bind(number)
                            .bind(offset)
                            .bind(bytes)
                            .execute(&mut *tx)
                            .await?;
                        offset += size;
                    }
                }
            }
            let summary = reader.summary()?;
            tx.commit().await?;
            Ok::<_, StoreError>(summary)
        }
        .await;
        match result {
            Ok(summary) => Ok(Self { database, summary }),
            Err(error) => {
                database.close().await.map_err(StoreError::from)?;
                Err(error.into())
            }
        }
    }

    pub fn summary(&self) -> &BundleSummary {
        &self.summary
    }

    /// Validate original history before any position or proof relocation. This
    /// does not bind destination workspaces, grant execution or publish Sessions.
    pub async fn validate_history(&mut self) -> Result<(), BundleError> {
        super::validation::validate(&mut self.database).await?;
        Ok(())
    }

    /// Await physical cleanup when the caller needs a completed teardown.
    pub async fn close(self) -> Result<(), BundleError> {
        self.database.close().await.map_err(StoreError::from)?;
        Ok(())
    }
}
