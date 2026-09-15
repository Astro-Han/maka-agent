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

use super::{Deployment, store};
use maka_event_log::{
    StoreError,
    root::{FileLease, RootOwner},
};
use maka_runtime_host::server::HostError;
use sqlx::{Connection, SqliteConnection};
use std::{path::Path, sync::Arc};

/// Pending intent never authorizes startup. The unchanged Active row does.
pub(super) async fn read(
    directory: &Path,
    lease: Arc<FileLease>,
) -> Result<(Deployment, Option<Deployment>), HostError> {
    let connection = store::open_writer(directory, lease, None).await?;
    let result = connection
        .run(|connection| Box::pin(load(connection)))
        .await;
    let closed = connection.close().await;
    let state = result?;
    closed?;
    Ok(state)
}

pub(super) async fn prepare(
    directory: &Path,
    lease: Arc<FileLease>,
    expected: Deployment,
    target: Deployment,
) -> Result<(), HostError> {
    validate_target(&expected, &target)?;
    let connection = store::open_writer(directory, lease, None).await?;
    let result = connection.run(move |connection| Box::pin(async move {
        let mut transaction = connection.begin().await?;
        let (active, pending) = load(&mut transaction).await?;
        if active != expected || pending.as_ref().is_some_and(|pending| pending != &target) {
            return Err(conflict());
        }
        sqlx::query("INSERT INTO deployment_update(singleton, target) VALUES (1, ?) ON CONFLICT(singleton) DO NOTHING")
            .bind(serde_json::to_string(&target)?)
            .execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    })).await;
    let closed = connection.close().await;
    result?;
    closed?;
    Ok(())
}

pub(super) async fn commit(
    directory: &Path,
    lease: Arc<FileLease>,
    owner: Arc<RootOwner>,
    expected: Deployment,
    target: Deployment,
) -> Result<(), HostError> {
    validate_target(&expected, &target)?;
    if owner.root_id() != expected.root_id || owner.canonical_path() != expected.root_path {
        return Err("update does not own the expected State Root".into());
    }
    // Keep both authorities in the worker through actual database close, including
    // a dropped caller, a failed COMMIT or a queued transaction rollback.
    let connection = store::open_writer(directory, lease, Some(owner)).await?;
    let result = connection
        .run(move |connection| {
            Box::pin(async move {
                let mut transaction = connection.begin().await?;
                let (active, pending) = load(&mut transaction).await?;
                if active != expected || pending.as_ref() != Some(&target) {
                    return Err(conflict());
                }
                sqlx::query("UPDATE deployment SET configuration = ? WHERE singleton = 1")
                    .bind(serde_json::to_string(&target)?)
                    .execute(&mut *transaction)
                    .await?;
                sqlx::query("DELETE FROM deployment_update WHERE singleton = 1")
                    .execute(&mut *transaction)
                    .await?;
                transaction.commit().await?;
                Ok(())
            })
        })
        .await;
    let closed = connection.close().await;
    result?;
    closed?;
    Ok(())
}

pub(super) fn validate_target(current: &Deployment, target: &Deployment) -> Result<(), StoreError> {
    let mut expected = target.clone();
    expected.executable = current.executable.clone();
    expected.sha256 = current.sha256.clone();
    expected.mode = current.mode;
    expected.websocket = current.websocket;
    expected.project_directory_roots = current.project_directory_roots.clone();
    expected.config_revision = current.config_revision;
    if &expected != current
        || current.config_revision >= 9_007_199_254_740_991
        || target.config_revision != current.config_revision + 1
        || (target.sha256 == current.sha256
            && target.mode == current.mode
            && target.websocket == current.websocket
            && target.project_directory_roots == current.project_directory_roots)
        || serde_json::to_vec(target)?.len() > 65_536
    {
        return Err(conflict());
    }
    Ok(())
}

pub(super) async fn load(
    connection: &mut SqliteConnection,
) -> Result<(Deployment, Option<Deployment>), StoreError> {
    let (active, pending): (String, Option<String>) = sqlx::query_as(
        "SELECT CASE WHEN length(CAST(configuration AS BLOB)) <= 65536 THEN configuration END, \
         (SELECT CASE WHEN length(CAST(target AS BLOB)) <= 65536 THEN target ELSE '' END FROM deployment_update WHERE singleton = 1) \
         FROM deployment WHERE singleton = 1"
    ).fetch_one(connection).await?;
    let active = serde_json::from_str(&active)?;
    let pending = pending
        .map(|value| serde_json::from_str(&value))
        .transpose()?;
    if let Some(target) = &pending {
        validate_target(&active, target)?;
    }
    Ok((active, pending))
}

fn conflict() -> StoreError {
    StoreError::InvalidTransition("deployment or pending update changed".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::{Mode, package};
    use maka_event_log::root::{self, RootNamespaces};

    #[tokio::test]
    async fn pending_intent_survives_reopen_and_cut_is_atomic_under_the_real_root_owner() {
        let temporary = tempfile::tempdir().unwrap();
        let namespaces = RootNamespaces {
            ownership: temporary.path().join("ownership"),
            control: temporary.path().join("control"),
        };
        let root_path = temporary.path().join("root");
        let owner = RootOwner::create(&root_path, &namespaces).unwrap();
        let directory = temporary.path().join("deployment");
        root::private_directory(&directory).unwrap();
        let lease = Arc::new(FileLease::acquire(&directory.join("executor.lock")).unwrap());
        let current = Deployment {
            deployment_id: uuid::Uuid::new_v4(),
            config_revision: 1,
            root_id: owner.root_id().into(),
            root_path: owner.canonical_path().into(),
            executable: package::path(&directory, &"a".repeat(64)),
            sha256: "a".repeat(64),
            mode: Mode::OnDemand,
            websocket: "127.0.0.1:0".parse().unwrap(),
            project_directory_roots: None,
            admission: super::super::Admission::Active,
        };
        store::install(&directory, lease.clone(), owner, current.clone())
            .await
            .unwrap();
        let mut target = current.clone();
        target.config_revision += 1;
        // A configuration-only target has the same executable, but still
        // requires the exact pending receipt and writer-held atomic cut.
        target.project_directory_roots = Some(Vec::new());
        prepare(&directory, lease.clone(), current.clone(), target.clone())
            .await
            .unwrap();
        // Old startup readers still see the sole authorized version while an
        // updater is absent. A different target cannot overwrite its intent.
        let store::Installation::Installed(active) = store::read(&directory).await.unwrap() else {
            panic!("pending intent must not change the startup reader contract");
        };
        assert_eq!(active, current);
        let mut other = target.clone();
        other.sha256 = "c".repeat(64);
        other.executable = package::path(&directory, &other.sha256);
        assert!(
            prepare(&directory, lease.clone(), current.clone(), other)
                .await
                .is_err()
        );
        assert_eq!(
            read(&directory, lease.clone()).await.unwrap(),
            (current.clone(), Some(target.clone()))
        );

        let writer = store::open_writer(&directory, lease.clone(), None)
            .await
            .unwrap();
        writer.run(|connection| Box::pin(async move {
            sqlx::raw_sql("CREATE TRIGGER reject_cut BEFORE DELETE ON deployment_update BEGIN SELECT RAISE(ABORT, 'cut failed'); END;")
                .execute(connection).await?;
            Ok(())
        })).await.unwrap();
        writer.close().await.unwrap();
        let owner = RootOwner::open(&root_path, &namespaces).unwrap();
        assert!(
            commit(
                &directory,
                lease.clone(),
                Arc::new(owner),
                current.clone(),
                target.clone()
            )
            .await
            .is_err()
        );
        assert_eq!(
            read(&directory, lease.clone()).await.unwrap(),
            (current.clone(), Some(target.clone()))
        );
        // Failed cut closed the SQL connection before returning writer authority.
        let owner = RootOwner::open(&root_path, &namespaces).unwrap();
        let writer = store::open_writer(&directory, lease.clone(), None)
            .await
            .unwrap();
        writer
            .run(|connection| {
                Box::pin(async move {
                    sqlx::query("DROP TRIGGER reject_cut")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .await
            .unwrap();
        writer.close().await.unwrap();
        commit(
            &directory,
            lease.clone(),
            Arc::new(owner),
            current,
            target.clone(),
        )
        .await
        .unwrap();
        // Uninstall cancels pending intent atomically, but retains a tombstone.
        // Only explicit reinstallation can grant a fresh deployment identity.
        let mut pending = target.clone();
        pending.config_revision += 1;
        pending.sha256 = "d".repeat(64);
        pending.executable = package::path(&directory, &pending.sha256);
        prepare(&directory, lease.clone(), target.clone(), pending)
            .await
            .unwrap();
        let owner = Arc::new(RootOwner::open(&root_path, &namespaces).unwrap());
        let revoked = store::change(
            &directory,
            lease.clone(),
            owner.clone(),
            target.clone(),
            store::Change::Revoke,
        )
        .await
        .unwrap();
        assert_eq!(revoked.config_revision, 3);
        assert!(revoked.require_active().is_err());
        assert_eq!(
            read(&directory, lease.clone()).await.unwrap(),
            (revoked.clone(), None)
        );
        // A stale operator cannot overwrite the tombstone after a lost response.
        assert!(
            store::change(
                &directory,
                lease.clone(),
                owner.clone(),
                target.clone(),
                store::Change::Revoke
            )
            .await
            .is_err()
        );
        let mut reinstalled = target;
        reinstalled.deployment_id = uuid::Uuid::new_v4();
        reinstalled.config_revision = 1;
        let active = store::change(
            &directory,
            lease.clone(),
            owner.clone(),
            revoked,
            store::Change::Reinstall(reinstalled.clone()),
        )
        .await
        .unwrap();
        assert_eq!(read(&directory, lease).await.unwrap(), (active, None));
        drop(owner);
        drop(RootOwner::open(&root_path, &namespaces).unwrap());
    }
}
