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

use super::Deployment;
use maka_event_log::{
    StoreError,
    connection::{ConnectionAuthority, OwnedConnection},
    root::{FileLease, RootOwner},
};
use maka_runtime_host::server::HostError;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{fs::File, path::Path, sync::Arc};

const DATABASE: &str = "deployment.sqlite";
const APPLICATION_ID: i64 = 0x4d414b44;
static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations/deployment");
// user_version describes the stable Active reader, not the SQLx migration count.

pub(super) enum Installation {
    Missing,
    Incomplete,
    Installed(Deployment),
}

/// No executor lock and no schema initialization: launchers can read while an
/// operator waits for their Ready acknowledgement.
pub(super) async fn read(directory: &Path) -> Result<Installation, HostError> {
    let path = directory.join(DATABASE);
    match path.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Installation::Missing);
        }
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_file() => return Err("invalid deployment database".into()),
        Ok(_) => {}
    }
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true))
            .await?;
    let result = async {
        let application: i64 = sqlx::query_scalar("PRAGMA application_id").fetch_one(&mut connection).await?;
        let version: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(&mut connection).await?;
        if version == 0 && application == APPLICATION_ID {
            return Ok(Installation::Incomplete);
        }
        if version == 0 && application == 0 {
            let tables: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")
                .fetch_one(&mut connection).await?;
            if tables == 0 { return Ok(Installation::Incomplete) }
        }
        if application != APPLICATION_ID || version != 1 {
            return Err("unsupported or incomplete deployment database".into());
        }
        let value: Option<String> = sqlx::query_scalar("SELECT CASE WHEN length(CAST(configuration AS BLOB)) <= 65536 THEN configuration END FROM deployment WHERE singleton = 1")
            .fetch_optional(&mut connection).await?;
        let Some(value) = value else { return Ok(Installation::Incomplete) };
        Ok::<_, HostError>(Installation::Installed(serde_json::from_str(&value)?))
    }.await;
    let closed = connection.close().await;
    let deployment = result?;
    closed?;
    Ok(deployment)
}

pub(super) async fn install(
    directory: &Path,
    lease: Arc<FileLease>,
    owner: RootOwner,
    deployment: Deployment,
) -> Result<Deployment, HostError> {
    let connection = open_writer(directory, lease, None).await?;
    let result = connection
        .run(move |connection| {
            Box::pin(async move {
                // The accepted database job owns Root until its actual commit, even if
                // the install caller disappears while awaiting the acknowledgement.
                owner.validate_current()?;
                let body = serde_json::to_string(&deployment)?;
                sqlx::query("INSERT INTO deployment(singleton, configuration) VALUES (1, ?)")
                    .bind(body)
                    .execute(connection)
                    .await?;
                drop(owner);
                Ok(deployment)
            })
        })
        .await;
    let closed = connection.close().await;
    let deployment = result?;
    closed?;
    Ok(deployment)
}

pub(super) async fn open_writer(
    directory: &Path,
    lease: Arc<FileLease>,
    root: Option<Arc<RootOwner>>,
) -> Result<OwnedConnection, HostError> {
    let path = directory.join(DATABASE);
    let file = private_file(&path)?;
    file.sync_all()?;
    #[cfg(unix)]
    File::open(directory)?.sync_all()?;
    Ok(OwnedConnection::open(
        SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false),
        ConnectionAuthority::Writer { lease, root },
        |connection| Box::pin(initialize(connection)),
    )
    .await?)
}

fn private_file(path: &Path) -> Result<File, HostError> {
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?
    };
    #[cfg(windows)]
    let file = {
        use maka_event_log::root::windows::{create_private_file, open_nofollow, validate_private};
        let file = match create_private_file(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                open_nofollow(path, true)?
            }
            Err(error) => return Err(error.into()),
        };
        validate_private(&file)?;
        file
    };
    if !file.metadata()?.is_file() {
        return Err("deployment database is not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata()?;
        if metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err("deployment database is not private and singly linked".into());
        }
    }
    #[cfg(windows)]
    if maka_event_log::root::windows::file_identity(&file)?.links != 1 {
        return Err("deployment database is hard-linked".into());
    }
    Ok(file)
}

async fn initialize(connection: &mut SqliteConnection) -> Result<(), StoreError> {
    let application: i64 = sqlx::query_scalar("PRAGMA application_id")
        .fetch_one(&mut *connection)
        .await?;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *connection)
        .await?;
    let tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !((application == 0 && version == 0 && tables == 0)
        || (application == APPLICATION_ID && (0..=1).contains(&version)))
    {
        return Err(StoreError::InvalidTransition(
            "unsupported deployment database".into(),
        ));
    }
    sqlx::raw_sql(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA application_id=1296124740;",
    )
    .execute(&mut *connection)
    .await?;
    MIGRATIONS.run_direct(None, connection, false).await?;
    Ok(())
}
