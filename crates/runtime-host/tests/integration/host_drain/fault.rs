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

use maka_event_log::{
    EventLog,
    root::{ROOT_DATABASE, RootNamespaces, RootOwner},
};
use maka_runtime_host::session::SessionConfiguration;
use serde_json::{Value, json};
use sqlx::Connection;
use std::{path::Path, sync::Arc};

#[derive(Clone, Copy)]
pub(super) enum Fault {
    Session,
    Catalog,
}

pub(super) async fn inject(root: &Path, fault: Fault) -> (&'static str, Value, &'static str) {
    let (database_name, trigger, operation, input, error_code) = match fault {
        Fault::Session => (
            ROOT_DATABASE,
            "CREATE TRIGGER drain_commit_failure AFTER UPDATE ON session_control
             BEGIN INSERT INTO drain_child VALUES (1); END;",
            "session.lifecycle.set",
            json!({"sessionId":"session","state":"archived"}),
            "commit_outcome_unknown",
        ),
        Fault::Catalog => (
            "configuration-rust.sqlite",
            "CREATE TRIGGER drain_commit_failure AFTER UPDATE ON connection_catalog
             BEGIN INSERT INTO drain_child VALUES (1); END;",
            "connection.catalog.create",
            json!({"expectedCatalogRevision":0,"connection":{
                "slug":"fixture","name":"Fixture","providerType":"openai",
                "enabled":true,"enabledModelIds":["gpt-5"]
            }}),
            "commit_outcome_unknown",
        ),
    };

    // SQLite accepts the UPDATE and fails only at COMMIT. This exercises the
    // production store-to-protocol classification, without host failpoints.
    let mut database = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(root.join(database_name)),
    )
    .await
    .unwrap();
    sqlx::raw_sql("CREATE TABLE drain_parent (id INTEGER PRIMARY KEY);
        CREATE TABLE drain_child (id INTEGER REFERENCES drain_parent(id) DEFERRABLE INITIALLY DEFERRED);")
        .execute(&mut database).await.unwrap();
    sqlx::raw_sql(trigger).execute(&mut database).await.unwrap();
    database.close().await.unwrap();

    (operation, input, error_code)
}

pub(super) async fn assert_recovered(root: &Path, ns: &RootNamespaces, fault: Fault) {
    let database_name = "configuration-rust.sqlite";
    if matches!(fault, Fault::Catalog) {
        let mut database = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(root.join(database_name)),
        )
        .await
        .unwrap();
        let (revision, connections): (i64, i64) = sqlx::query_as(
            "SELECT revision,(SELECT count(*) FROM connections) FROM connection_catalog WHERE singleton=1",
        )
        .fetch_one(&mut database)
        .await
        .unwrap();
        assert_eq!(
            revision, 0,
            "failed mutation must roll back catalog revision"
        );
        assert_eq!(
            connections, 0,
            "failed mutation cannot publish a connection"
        );
        sqlx::query("DROP TRIGGER drain_commit_failure")
            .execute(&mut database)
            .await
            .unwrap();
        database.close().await.unwrap();
    }
    let owner = RootOwner::open(root, ns).unwrap();
    let owner = Arc::new(owner);
    let log = EventLog::for_root(owner.clone()).await.unwrap();
    let session = log
        .get_session::<SessionConfiguration>("session")
        .await
        .unwrap()
        .unwrap();
    assert!(!session.archived);
    assert_eq!(session.revision, 1);
    assert!(
        log.prefix(100, 1024 * 1024)
            .await
            .unwrap()
            .events
            .is_empty()
    );
    log.shutdown().await.unwrap();
    if matches!(fault, Fault::Catalog) {
        let configuration = maka_config::ConfigurationStore::for_root(owner)
            .await
            .unwrap();
        let recovered = configuration.catalog().await.unwrap();
        assert_eq!(recovered.revision, 0);
        assert!(recovered.connections.is_empty());
        configuration.shutdown().await.unwrap();
    }
}
