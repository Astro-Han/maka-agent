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

use std::collections::BTreeMap;

use maka_plugins::package::{MAX_FILE_BYTES, MAX_FILES, MAX_PACKAGE_BYTES, Package};
use sqlx::SqliteConnection;

use super::invalid;
use crate::StoreError;

pub(super) async fn install(
    connection: &mut SqliteConnection,
    package: &Package,
) -> Result<(), StoreError> {
    let inserted = sqlx::query("INSERT OR IGNORE INTO plugin_package_blobs(digest) VALUES (?)")
        .bind(package.digest())
        .execute(&mut *connection)
        .await?;
    if inserted.rows_affected() == 1 {
        for (path, bytes) in package.files() {
            sqlx::query("INSERT INTO plugin_package_files(digest, path, payload) VALUES (?, ?, ?)")
                .bind(package.digest())
                .bind(path)
                .bind(bytes)
                .execute(&mut *connection)
                .await?;
        }
    }
    sqlx::query(
        "INSERT INTO plugin_packages(id, digest) VALUES (?, ?)
         ON CONFLICT(id) DO UPDATE SET digest = excluded.digest",
    )
    .bind(&package.manifest().id)
    .bind(package.digest())
    .execute(connection)
    .await?;
    Ok(())
}

pub(super) async fn read(
    connection: &mut SqliteConnection,
    digest: &str,
) -> Result<Package, StoreError> {
    let (count, total, largest): (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(length(payload)),0), COALESCE(MAX(length(payload)),0)
         FROM plugin_package_files WHERE digest = ?",
    )
    .bind(digest)
    .fetch_one(&mut *connection)
    .await?;
    if count == 0
        || count > MAX_FILES as i64
        || total > MAX_PACKAGE_BYTES as i64
        || largest > MAX_FILE_BYTES as i64
    {
        return Err(invalid("stored package exceeds file or byte limits"));
    }
    let files: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT path, payload FROM plugin_package_files WHERE digest = ? ORDER BY path",
    )
    .bind(digest)
    .fetch_all(connection)
    .await?;
    let package = Package::new(files.into_iter().collect::<BTreeMap<_, _>>())
        .map_err(|error| invalid(&error.to_string()))?;
    if package.digest() != digest {
        return Err(invalid("stored package digest mismatch"));
    }
    Ok(package)
}
