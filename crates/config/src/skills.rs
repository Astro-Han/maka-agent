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

use crate::{ConfigError, ConfigurationStore, Result, TransactionMode};
use maka_runtime::{configuration::validation::MAX_SAFE_INTEGER, skills::SkillPreference};
use sqlx::SqliteConnection;
use std::collections::BTreeMap;

const MAX_PREFERENCES: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPreferences {
    pub revision: u64,
    pub entries: BTreeMap<String, SkillPreference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreferenceUpdate {
    Committed { revision: u64 },
    Conflict { actual_revision: u64 },
}

impl ConfigurationStore {
    pub async fn skill_preferences(&self) -> Result<SkillPreferences> {
        self.transaction(TransactionMode::Deferred, |connection| {
            Box::pin(read(connection))
        })
        .await
    }

    /// The catalog's later filesystem revision check must include this control
    /// revision; a stale preference writer cannot overwrite a newer decision.
    pub async fn set_skill_preference(
        &self,
        expected_revision: u64,
        reference: String,
        preference: SkillPreference,
    ) -> Result<PreferenceUpdate> {
        if expected_revision > MAX_SAFE_INTEGER || !valid_reference(&reference) {
            return Err(ConfigError::Invalid(
                "invalid skill preference target or revision".into(),
            ));
        }
        self.transaction(TransactionMode::Immediate, move |connection| Box::pin(async move {
            let current = read(connection).await?;
            if current.revision != expected_revision {
                return Ok(PreferenceUpdate::Conflict { actual_revision: current.revision });
            }
            if current.entries.get(&reference).copied().unwrap_or_default() == preference {
                return Ok(PreferenceUpdate::Committed { revision: current.revision });
            }
            if current.revision == MAX_SAFE_INTEGER
                || (current.entries.len() == MAX_PREFERENCES && !current.entries.contains_key(&reference))
            {
                return Err(ConfigError::Invalid("skill preference capacity exhausted".into()));
            }
            if preference == SkillPreference::default() {
                sqlx::query("DELETE FROM skill_preferences WHERE ref = ?")
                    .bind(reference).execute(&mut *connection).await?;
            } else {
                sqlx::query("INSERT INTO skill_preferences(ref, enabled, pinned) VALUES (?, ?, ?) ON CONFLICT(ref) DO UPDATE SET enabled = excluded.enabled, pinned = excluded.pinned")
                    .bind(reference).bind(preference.enabled).bind(preference.pinned)
                    .execute(&mut *connection).await?;
            }
            let revision = current.revision + 1;
            sqlx::query("UPDATE skill_preferences_revision SET revision = ? WHERE singleton = 1")
                .bind(revision as i64).execute(connection).await?;
            Ok(PreferenceUpdate::Committed { revision })
        })).await
    }
}

async fn read(connection: &mut SqliteConnection) -> Result<SkillPreferences> {
    let revisions: Vec<(i64, i64)> =
        sqlx::query_as("SELECT singleton, revision FROM skill_preferences_revision LIMIT 2")
            .fetch_all(&mut *connection)
            .await?;
    let [(1, revision)] = revisions.as_slice() else {
        return Err(ConfigError::UnsupportedDatabase);
    };
    let revision = u64::try_from(*revision)
        .ok()
        .filter(|r| *r <= MAX_SAFE_INTEGER)
        .ok_or(ConfigError::UnsupportedDatabase)?;
    let invalid: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_preferences WHERE length(CAST(ref AS BLOB)) NOT BETWEEN 1 AND 512 OR enabled NOT IN (0, 1) OR pinned NOT IN (0, 1)"
    ).fetch_one(&mut *connection).await?;
    if invalid != 0 {
        return Err(ConfigError::UnsupportedDatabase);
    }
    let rows: Vec<(String, bool, bool)> =
        sqlx::query_as("SELECT ref, enabled, pinned FROM skill_preferences ORDER BY ref LIMIT ?")
            .bind((MAX_PREFERENCES + 1) as i64)
            .fetch_all(connection)
            .await?;
    if rows.len() > MAX_PREFERENCES
        || rows
            .iter()
            .any(|(reference, _, _)| !valid_reference(reference))
    {
        return Err(ConfigError::UnsupportedDatabase);
    }
    Ok(SkillPreferences {
        revision,
        entries: rows
            .into_iter()
            .map(|(reference, enabled, pinned)| (reference, SkillPreference { enabled, pinned }))
            .collect(),
    })
}

fn valid_reference(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 512
        && !reference
            .chars()
            .any(|c| matches!(c, '\u{0000}'..='\u{001f}' | '\u{007f}'))
}
