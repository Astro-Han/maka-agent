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

use super::{
    BundleError, BundleSummary,
    format::{Blob, MAX_BLOB_BYTES, Record},
    reader::Reader,
};
use crate::StoreError;
use maka_runtime::artifact::content_digest;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tokio::io::AsyncRead;

static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./src/bundle/migrations");

pub(super) async fn payload(
    staged: &mut SqliteConnection,
    number: i64,
    descriptor: &Blob,
) -> Result<Vec<u8>, StoreError> {
    if descriptor.bytes() > MAX_BLOB_BYTES {
        return Err(StoreError::PrefixTooLarge);
    }
    let mut payload = Vec::with_capacity(descriptor.bytes() as usize);
    while payload.len() < descriptor.bytes() as usize {
        let chunk: Vec<u8> =
            sqlx::query_scalar("SELECT payload FROM chunks WHERE frame=? AND offset=?")
                .bind(number)
                .bind(payload.len() as i64)
                .fetch_one(&mut *staged)
                .await?;
        if chunk.is_empty() || chunk.len() > descriptor.bytes() as usize - payload.len() {
            return Err(StoreError::InvalidTransition(
                "staged blob length differs".into(),
            ));
        }
        payload.extend_from_slice(&chunk);
    }
    if content_digest(&payload) != descriptor.digest() {
        return Err(StoreError::InvalidTransition(
            "staged blob digest differs".into(),
        ));
    }
    Ok(payload)
}

/// Private decoded transfer, not yet accepted history. Only Host-authored SQL
/// opens this staging database; input never supplies a SQLite file or schema.
/// Dropping it also closes SQLite's automatically deleted temporary database.
pub struct StagedBundle {
    pub(super) database: SqliteConnection,
    pub(super) summary: BundleSummary,
    history_validated: bool,
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
            Ok(summary) => Ok(Self {
                database,
                summary,
                history_validated: false,
            }),
            Err(error) => {
                database.close().await.map_err(StoreError::from)?;
                Err(error.into())
            }
        }
    }

    pub fn summary(&self) -> &BundleSummary {
        &self.summary
    }

    /// Source metadata is untrusted historical data, not destination authority.
    /// Read one configuration at a time rather than materializing the catalog.
    pub async fn configuration<T: serde::de::DeserializeOwned>(
        &mut self,
        session: &str,
    ) -> Result<T, BundleError> {
        let json: String = sqlx::query_scalar(
            "SELECT record_json FROM frames WHERE kind='session' AND json_extract(record_json,'$.id')=?",
        )
        .bind(session)
        .fetch_one(&mut self.database)
        .await
        .map_err(StoreError::from)?;
        let Record::Session { configuration, .. } =
            serde_json::from_str(&json).map_err(StoreError::from)?
        else {
            unreachable!()
        };
        serde_json::from_value(configuration)
            .map_err(StoreError::from)
            .map_err(Into::into)
    }

    pub async fn artifact_count(&mut self) -> Result<u64, BundleError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM frames WHERE kind='blob' AND json_extract(record_json,'$.resource')='artifact'",
        )
        .fetch_one(&mut self.database)
        .await
        .map_err(StoreError::from)?;
        crate::sequence_number(count).map_err(Into::into)
    }

    /// Validate original history before any position or proof relocation. This
    /// does not bind destination workspaces, grant execution or publish Sessions.
    pub async fn validate_history(&mut self) -> Result<(), BundleError> {
        // Staged source bytes are private and immutable after read. Callers may
        // validate before acquiring destination admission without repeating it.
        if !self.history_validated {
            super::validation::validate(&mut self.database).await?;
            self.history_validated = true;
        }
        Ok(())
    }

    /// Await physical cleanup when the caller needs a completed teardown.
    pub async fn close(self) -> Result<(), BundleError> {
        self.database.close().await.map_err(StoreError::from)?;
        Ok(())
    }
}
