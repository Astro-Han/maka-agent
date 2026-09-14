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

use super::*;
use crate::sessions::{MAX_CONFIGURATION_BYTES, advance_catalog};
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug)]
pub struct ProjectRelinkContext {
    pub project_id: String,
    pub previous_ids: Vec<String>,
    pub absorbed_ids: Vec<String>,
    pub destination_path: String,
}

#[derive(Debug)]
pub struct ProjectRelink {
    pub project: ProjectRecord,
    pub updated_session_ids: Vec<String>,
}

impl EventLog {
    /// Project aliases, locations and Session membership commit together.
    /// The domain closure updates validated, typed Session control metadata;
    /// runtime facts, creation fingerprints and activity timestamps never move.
    pub async fn relink_project<T, F>(
        &self,
        id: &str,
        registration: ProjectRegistration,
        now: u64,
        mut reassign: F,
    ) -> Result<ProjectRelink>
    where
        T: Serialize + DeserializeOwned + Send + 'static,
        F: FnMut(&mut T, &ProjectRelinkContext) -> Result<()> + Send + 'static,
    {
        self.validate_root()?;
        validate_id(id)?;
        validate_registration(&registration)?;
        let now = timestamp(now)?;
        let id = id.to_owned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    let project = read(&mut tx, &id).await?.ok_or(ProjectError::NotFound)?;
                    let conflict_id: Option<String> = sqlx::query_scalar(
                        "SELECT id FROM projects WHERE identity = ? AND id != ?",
                    )
                    .bind(&registration.identity)
                    .bind(&project.id)
                    .fetch_optional(&mut *tx)
                    .await?;
                    let conflict = match conflict_id {
                        Some(id) => Some(read(&mut tx, &id).await?.ok_or(ProjectError::NotFound)?),
                        None => None,
                    };
                    let context = ProjectRelinkContext {
                        project_id: project.id.clone(),
                        previous_ids: std::iter::once(project.id.clone())
                            .chain(project.aliases.iter().cloned())
                            .collect(),
                        absorbed_ids: conflict
                            .iter()
                            .flat_map(|record| {
                                std::iter::once(record.id.clone())
                                    .chain(record.aliases.iter().cloned())
                            })
                            .collect(),
                        destination_path: registration.path.clone(),
                    };
                    let updated_session_ids =
                        reassign_sessions(&mut tx, &context, &mut reassign).await?;
                    sqlx::query("DELETE FROM project_locations WHERE project_id = ?")
                        .bind(&project.id)
                        .execute(&mut *tx)
                        .await?;
                    if let Some(conflict) = &conflict {
                        // Move identities before deleting their former owner; the unique
                        // namespace prevents alias collisions throughout the transaction.
                        sqlx::query(
                            "UPDATE project_identities SET project_id = ? WHERE project_id = ?",
                        )
                        .bind(&project.id)
                        .bind(&conflict.id)
                        .execute(&mut *tx)
                        .await?;
                        sqlx::query(
                            "INSERT INTO project_locations
                    SELECT ?, path, is_worktree, last_used_at FROM project_locations
                    WHERE project_id = ? AND path != ?",
                        )
                        .bind(&project.id)
                        .bind(&conflict.id)
                        .bind(&registration.path)
                        .execute(&mut *tx)
                        .await?;
                        sqlx::query("DELETE FROM projects WHERE id = ?")
                            .bind(&conflict.id)
                            .execute(&mut *tx)
                            .await?;
                    }
                    sqlx::query("INSERT INTO project_locations VALUES (?, ?, ?, ?)")
                        .bind(&project.id)
                        .bind(&registration.path)
                        .bind(registration.is_worktree)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                    let last_used = now.max(project.last_used_at as i64).max(
                        conflict
                            .as_ref()
                            .map_or(0, |record| record.last_used_at as i64),
                    );
                    sqlx::query("UPDATE projects SET identity = ?, last_used_at = ? WHERE id = ?")
                        .bind(&registration.identity)
                        .bind(last_used)
                        .bind(&project.id)
                        .execute(&mut *tx)
                        .await?;
                    let project = read(&mut tx, &project.id)
                        .await?
                        .ok_or(ProjectError::NotFound)?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    Ok(ProjectRelink {
                        project,
                        updated_session_ids,
                    })
                })
            })
            .await
    }
}

async fn reassign_sessions<T, F>(
    connection: &mut SqliteConnection,
    context: &ProjectRelinkContext,
    reassign: &mut F,
) -> Result<Vec<String>>
where
    T: Serialize + DeserializeOwned,
    F: FnMut(&mut T, &ProjectRelinkContext) -> Result<()>,
{
    let mut updated = Vec::new();
    // Bounded row batches avoid loading all Session configurations at once.
    let mut cursor = String::new();
    loop {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, configuration FROM session_control WHERE id > ? ORDER BY id LIMIT 32",
        )
        .bind(&cursor)
        .fetch_all(&mut *connection)
        .await?;
        if rows.is_empty() {
            break;
        }
        for (id, original) in rows {
            let mut configuration: T = serde_json::from_str(&original)?;
            // Compare normalized encodings, not the input's incidental key order.
            let before = serde_json::to_string(&configuration)?;
            reassign(&mut configuration, context)?;
            let after = serde_json::to_string(&configuration)?;
            if after != before {
                if after.len() > MAX_CONFIGURATION_BYTES {
                    return Err(invalid("relinked Session configuration exceeds capacity"));
                }
                if sqlx::query(
                    "UPDATE session_control SET configuration = ?, revision = revision + 1
                    WHERE id = ? AND revision < ?",
                )
                .bind(after)
                .bind(&id)
                .bind(MAX_SAFE_INTEGER as i64)
                .execute(&mut *connection)
                .await?
                .rows_affected()
                    != 1
                {
                    return Err(invalid("relinked Session revision exhausted"));
                }
                updated.push(id.clone());
            }
            cursor = id;
        }
    }
    if !updated.is_empty() {
        advance_catalog(connection).await?;
    }
    Ok(updated)
}
