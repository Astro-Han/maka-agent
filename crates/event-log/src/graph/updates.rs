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
use maka_graph::{
    GraphId, WorkId,
    schedule::{CommittedUpdate, Update, Work},
};
use maka_runtime::event::{Fact, RuntimeEvent};
use sqlx::{Connection, SqliteConnection};

impl EventLog {
    /// Work instructions are immutable facts, independent of a live coordinator.
    pub async fn graph_work(
        &self,
        root: &str,
        graph: &GraphId,
        work: &WorkId,
    ) -> Result<Option<Work>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(root)?;
        let (root, graph, work) = (root.to_owned(), graph.clone(), work.clone());
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let value: Option<String> = sqlx::query_scalar(
                        "SELECT work.value FROM graph_epochs AS epoch
                 JOIN graph_updates AS update_row ON update_row.graph_id = epoch.graph_id,
                 json_each(update_row.update_json, '$.addWork') AS work
                 WHERE epoch.root_session_id = ?1 AND epoch.graph_id = ?2
                 AND json_extract(work.value, '$.workId') = ?3 LIMIT 1",
                    )
                    .bind(root)
                    .bind(graph.as_str())
                    .bind(work.as_str())
                    .fetch_optional(connection)
                    .await?;
                    value
                        .map(|json| serde_json::from_str(&json).map_err(StoreError::from))
                        .transpose()
                })
            })
            .await
    }

    /// A committed decision is authoritative even if returning its Tool result
    /// later fails. Retrying that call recovers the decision, not a second effect.
    pub async fn commit_graph_update(
        &self,
        update: Update,
        expected_revision: u64,
        now: u64,
    ) -> Result<CommittedUpdate, StoreError> {
        self.validate_root()?;
        update.validate().map_err(invalid)?;
        crate::sessions::validate_time(now)?;
        if expected_revision >= (1 << 53) - 1 {
            return Err(invalid("invalid schedule revision"));
        }
        let fingerprint = update.fingerprint().map_err(invalid)?;
        let identity = update.identity();
        let json = serde_json::to_string(&update)?;
        if json.len() > 8 * 1024 * 1024 {
            return Err(invalid("schedule update exceeds byte limit"));
        }
        let commits = self.commits.clone();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            if let Some((previous, stored)) = read(&mut tx, &update.graph_id, &identity).await? {
                if stored != fingerprint { return Err(StoreError::EventConflict); }
                return Ok(previous);
            }
            let state = control(&mut tx, &update.graph_id).await?.ok_or_else(|| invalid("graph not found"))?;
            if state.epoch.root_session_id != update.source.invocation.session_id
                || current(&mut tx, &state.epoch.root_session_id).await?.as_ref() != Some(&update.graph_id)
                || state.schedule_revision != expected_revision {
                return Err(StoreError::EventConflict);
            }
            if state.closed() { return Err(invalid("graph is closed")); }
            let archived: bool = sqlx::query_scalar("SELECT archived FROM session_control WHERE id = ?")
                .bind(&state.epoch.root_session_id).fetch_one(&mut *tx).await?;
            if archived { return Err(invalid("graph Session is archived")); }
            let dispatch: Option<String> = sqlx::query_scalar(
                "SELECT event_json FROM runtime_events WHERE invocation_id = ? AND operation_id = ? AND kind = 'tool_dispatched'"
            ).bind(&update.source.invocation.invocation_id).bind(&update.source.operation_id).fetch_optional(&mut *tx).await?;
            let dispatch: RuntimeEvent = serde_json::from_str(&dispatch.ok_or_else(|| invalid("schedule update has no admitted Tool call"))?)?;
            if dispatch.invocation != update.source.invocation
                || !matches!(&dispatch.fact, Fact::ToolDispatched { name, .. } if name == "update_agent_graph") {
                return Err(invalid("schedule update source is not its Graph control Tool"));
            }
            let sealed: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE invocation_id = ?1
                 AND (kind = 'invocation_ended' OR (kind = 'tool_settled' AND operation_id = ?2)))"
            ).bind(&update.source.invocation.invocation_id).bind(&update.source.operation_id).fetch_one(&mut *tx).await?;
            if sealed { return Err(StoreError::Sealed); }
            let work_count: i64 = sqlx::query_scalar(
                "SELECT COALESCE(SUM(json_array_length(update_json, '$.addWork')), 0) FROM graph_updates WHERE graph_id = ?"
            ).bind(update.graph_id.as_str()).fetch_one(&mut *tx).await?;
            if work_count + update.add_work.len() as i64 > 1024 {
                return Err(invalid("graph epoch exceeds 1024 work items"));
            }
            for work in &update.add_work {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM graph_updates, json_each(update_json, '$.addWork') AS work
                     WHERE graph_id = ? AND json_extract(work.value, '$.workId') = ?)"
                ).bind(update.graph_id.as_str()).bind(work.work_id.as_str()).fetch_one(&mut *tx).await?;
                if exists { return Err(StoreError::EventConflict); }
            }
            let revision = expected_revision + 1;
            sqlx::query("INSERT INTO graph_updates (graph_id, update_id, revision, fingerprint, update_json, committed_at) VALUES (?, ?, ?, ?, ?, ?)")
                .bind(update.graph_id.as_str()).bind(identity).bind(revision as i64).bind(fingerprint).bind(json).bind(now as i64)
                .execute(&mut *tx).await?;
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            commits.send_modify(|_| {});
            Ok(CommittedUpdate { update, revision, committed_at: now })
        })).await
    }

    /// Contiguous pages; callers freeze `through` from graph_control before replay.
    pub async fn graph_updates(
        &self,
        graph: &GraphId,
        after: u64,
        through: u64,
    ) -> Result<Vec<CommittedUpdate>, StoreError> {
        self.validate_root()?;
        if after > through || through > (1 << 53) - 1 {
            return Err(invalid("invalid schedule cursor"));
        }
        let graph = graph.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
                        "WITH candidates AS (
                            SELECT revision, byte_count FROM graph_updates
                            WHERE graph_id = ?1 AND revision > ?2 AND revision <= ?3 ORDER BY revision LIMIT 16
                         ), bounded AS (
                            SELECT revision, ROW_NUMBER() OVER (ORDER BY revision) AS position,
                                SUM(byte_count) OVER (ORDER BY revision) AS bytes FROM candidates
                         ) SELECT update_json, revision, committed_at FROM graph_updates
                           WHERE graph_id = ?1 AND revision IN (
                               SELECT revision FROM bounded WHERE position = 1 OR bytes <= 2097152
                           ) ORDER BY revision",
                    )
                    .bind(graph.as_str())
                    .bind(after as i64)
                    .bind(through as i64)
                    .fetch_all(connection)
                    .await?;
                    rows.into_iter().map(decode).collect()
                })
            })
            .await
    }
}

async fn read(
    connection: &mut SqliteConnection,
    graph: &GraphId,
    identity: &str,
) -> Result<Option<(CommittedUpdate, String)>, StoreError> {
    let row: Option<(String, i64, i64, String)> = sqlx::query_as(
        "SELECT update_json, revision, committed_at, fingerprint FROM graph_updates WHERE graph_id = ? AND update_id = ?"
    ).bind(graph.as_str()).bind(identity).fetch_optional(connection).await?;
    row.map(|(json, revision, now, fingerprint)| Ok((decode((json, revision, now))?, fingerprint)))
        .transpose()
}

pub(super) fn decode(
    (json, revision, now): (String, i64, i64),
) -> Result<CommittedUpdate, StoreError> {
    let update: Update = serde_json::from_str(&json)?;
    update.validate().map_err(invalid)?;
    Ok(CommittedUpdate {
        update,
        revision: u64::try_from(revision).map_err(invalid)?,
        committed_at: u64::try_from(now).map_err(invalid)?,
    })
}
