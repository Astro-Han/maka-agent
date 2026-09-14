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

use maka_config::{ConfigurationStore, skills::PreferenceUpdate};
use maka_event_log::root::{RootNamespaces, RootOwner};
use maka_runtime::skills::SkillPreference;
use std::sync::Arc;

#[tokio::test]
async fn root_preferences_are_atomic_revisioned_and_corruption_is_not_default_enablement() {
    let temporary = tempfile::tempdir().unwrap();
    let owner = Arc::new(
        RootOwner::create(
            &temporary.path().join("root"),
            &RootNamespaces {
                ownership: temporary.path().join("owners"),
                control: temporary.path().join("control"),
            },
        )
        .unwrap(),
    );
    let legacy = owner.canonical_path().join(".maka");
    std::fs::create_dir(&legacy).unwrap();
    let legacy = legacy.join("skills-state.json");
    std::fs::write(&legacy, "original TS state is not an input").unwrap();
    let store = ConfigurationStore::for_root(owner.clone()).await.unwrap();
    let initial = store.skill_preferences().await.unwrap();
    assert_eq!(initial.revision, 0);
    assert!(initial.entries.is_empty());
    let reference = "project:maka:review".to_owned();
    let disabled = SkillPreference {
        enabled: false,
        pinned: true,
    };
    assert_eq!(
        store
            .set_skill_preference(0, reference.clone(), disabled)
            .await
            .unwrap(),
        PreferenceUpdate::Committed { revision: 1 }
    );
    assert_eq!(
        store
            .set_skill_preference(0, reference.clone(), SkillPreference::default())
            .await
            .unwrap(),
        PreferenceUpdate::Conflict { actual_revision: 1 }
    );
    assert_eq!(
        store
            .set_skill_preference(1, reference.clone(), disabled)
            .await
            .unwrap(),
        PreferenceUpdate::Committed { revision: 1 }
    );
    store.close().await.unwrap();
    let store = ConfigurationStore::for_root(owner.clone()).await.unwrap();
    assert_eq!(
        store.skill_preferences().await.unwrap().entries[&reference],
        disabled
    );
    assert_eq!(
        store
            .set_skill_preference(1, reference.clone(), SkillPreference::default())
            .await
            .unwrap(),
        PreferenceUpdate::Committed { revision: 2 }
    );
    let empty = store.skill_preferences().await.unwrap();
    assert_eq!(empty.revision, 2);
    assert!(empty.entries.is_empty());

    let db = rusqlite::Connection::open(owner.canonical_path().join("configuration-rust.sqlite"))
        .unwrap();
    db.execute_batch(
        "PRAGMA ignore_check_constraints = ON;
        INSERT INTO skill_preferences VALUES ('project:maka:review', 7, 0);",
    )
    .unwrap();
    assert!(store.skill_preferences().await.is_err());
    assert!(
        store
            .set_skill_preference(2, reference, disabled)
            .await
            .is_err()
    );
    assert_eq!(
        db.query_row(
            "SELECT revision FROM skill_preferences_revision",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        std::fs::read_to_string(legacy).unwrap(),
        "original TS state is not an input"
    );
    store.close().await.unwrap();
}
