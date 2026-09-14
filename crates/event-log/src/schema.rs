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

use sqlx::SqliteConnection;

use crate::StoreError;

static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub(crate) async fn initialize_connection(
    connection: &mut SqliteConnection,
) -> Result<(), StoreError> {
    let application_id: i64 = sqlx::query_scalar("PRAGMA application_id")
        .fetch_one(&mut *connection)
        .await?;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *connection)
        .await?;
    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !((application_id == 0 && version == 0 && tables == 0)
        || (application_id == 0x4d414b52 && matches!(version, 0..=17)))
    {
        return Err(StoreError::UnsupportedDatabase);
    }
    // Establish Rust identity before SQLx creates its migration ledger, so a
    // crash before migration 1 is distinguishable from an unrelated database.
    sqlx::raw_sql("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")
        .execute(&mut *connection)
        .await?;
    if application_id == 0 {
        sqlx::query("PRAGMA application_id = 1296124754")
            .execute(&mut *connection)
            .await?;
    }
    crate::sqlite_functions::register(connection).await?;
    MIGRATIONS.run_direct(None, &mut *connection, false).await?;
    crate::message_sources::initialize(connection).await?;
    crate::transcript::initialize(connection).await?;
    crate::sessions::initialize_execution(connection).await?;
    Ok(())
}
