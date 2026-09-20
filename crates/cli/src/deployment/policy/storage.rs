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

use super::{Deployment, Record};
use crate::deployment::{store, updates};
use maka_event_log::{StoreError, root::FileLease};
use maka_runtime_host::server::HostError;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{path::Path, sync::Arc};

pub(in crate::deployment) async fn read(directory: &Path) -> Result<Record, HostError> {
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(directory.join("deployment.sqlite"))
            .read_only(true),
    )
    .await?;
    let result = load(&mut connection).await;
    let closed = connection.close().await;
    let record = result?;
    closed?;
    Ok(record)
}

async fn load(connection: &mut SqliteConnection) -> Result<Record, StoreError> {
    let value: Option<String> = sqlx::query_scalar(
        "SELECT configuration FROM deployment_update_policy WHERE singleton = 1",
    )
    .fetch_optional(connection)
    .await?;
    Ok(value
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .unwrap_or_default())
}

pub(in crate::deployment) async fn write(
    directory: &Path,
    lease: Arc<FileLease>,
    deployment: &Deployment,
    previous: &Record,
    next: &Record,
) -> Result<(), HostError> {
    let (deployment, previous, next) = (deployment.clone(), previous.clone(), next.clone());
    let connection = store::open_writer(directory, lease, None).await?;
    let result = connection.run(move |connection| Box::pin(async move {
        let mut transaction = connection.begin().await?;
        let (active, _) = updates::load(&mut transaction).await?;
        if active != deployment || load(&mut transaction).await? != previous {
            return Err(StoreError::InvalidTransition("deployment or update policy changed".into()))
        }
        sqlx::query("INSERT INTO deployment_update_policy(singleton, configuration) VALUES (1, ?) ON CONFLICT(singleton) DO UPDATE SET configuration = excluded.configuration")
            .bind(serde_json::to_string(&next)?).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    })).await;
    let closed = connection.close().await;
    result?;
    closed?;
    Ok(())
}
