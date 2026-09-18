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
use super::{control, current, invalid};
use crate::{EventLog, StoreError};
use maka_graph::{GraphId, WorkId, control::Intent};
use sqlx::Connection;

impl EventLog {
    /// Freeze the concrete execution input before contacting Host admission.
    /// Identical recovery never changes the original input or operation ID.
    pub async fn commit_graph_intent(&self, intent: Intent) -> Result<Intent, StoreError> {
        self.validate_root()?;
        intent.validate().map_err(invalid)?;
        let encoded = serde_json::to_string(&intent)?;
        let commits = self.commits.clone();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let existing: Option<String> = sqlx::query_scalar("SELECT intent_json FROM graph_intents WHERE graph_id = ? AND work_id = ?")
                .bind(intent.graph_id.as_str()).bind(intent.work_id.as_str()).fetch_optional(&mut *tx).await?;
            if let Some(existing) = existing {
                let previous: Intent = serde_json::from_str(&existing)?;
                if previous != intent { return Err(StoreError::EventConflict); }
                return Ok(previous);
            }
            let state = control(&mut tx, &intent.graph_id).await?.ok_or_else(|| invalid("graph not found"))?;
            if state.closed() { return Err(invalid("graph is closed")); }
            if state.schedule_revision != intent.schedule_revision
                || current(&mut tx, &state.epoch.root_session_id).await?.as_ref() != Some(&intent.graph_id) {
                return Err(StoreError::EventConflict);
            }
            let introduced: Option<i64> = sqlx::query_scalar(
                "SELECT revision FROM graph_updates, json_each(update_json, '$.addWork') AS work
                 WHERE graph_id = ? AND json_extract(work.value, '$.workId') = ?"
            ).bind(intent.graph_id.as_str()).bind(intent.work_id.as_str()).fetch_optional(&mut *tx).await?;
            let introduced = introduced.ok_or_else(|| invalid("work not found"))?;
            // Inspect only relevant control facts; do not replay the whole graph
            // into Rust while holding the shared event-log writer.
            let stopped: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM graph_updates WHERE graph_id = ?1 AND revision >= ?2 AND (
                    EXISTS(SELECT 1 FROM json_each(update_json, '$.stop') AS stop
                           WHERE json_extract(stop.value, '$.targetId') IN (?3, ?4))
                    OR EXISTS(SELECT 1 FROM json_each(update_json, '$.addWork') AS replacement
                              WHERE json_extract(replacement.value, '$.replaces') IN (?3, ?4))))"
            ).bind(intent.graph_id.as_str()).bind(introduced).bind(intent.work_id.as_str()).bind(intent.operator_id.as_str())
                .fetch_one(&mut *tx).await?;
            if stopped { return Err(invalid("work is stopped or superseded")); }
            if intent.request.session_id == state.epoch.root_session_id {
                return Err(invalid("Graph work cannot run in its supervisor Session"));
            }
            sqlx::query("INSERT INTO graph_intents VALUES (?, ?, ?)")
                .bind(intent.graph_id.as_str()).bind(intent.work_id.as_str()).bind(encoded).execute(&mut *tx).await?;
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            commits.send_modify(|_| {});
            Ok(intent)
        })).await
    }

    pub async fn graph_intent(
        &self,
        graph: &GraphId,
        work: &WorkId,
    ) -> Result<Option<Intent>, StoreError> {
        self.validate_root()?;
        let graph = graph.clone();
        let work = work.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let json: Option<String> = sqlx::query_scalar(
                        "SELECT intent_json FROM graph_intents WHERE graph_id = ? AND work_id = ?",
                    )
                    .bind(graph.as_str())
                    .bind(work.as_str())
                    .fetch_optional(connection)
                    .await?;
                    json.map(|json| {
                        let intent: Intent = serde_json::from_str(&json)?;
                        intent.validate().map_err(invalid)?;
                        Ok(intent)
                    })
                    .transpose()
                })
            })
            .await
    }
}
