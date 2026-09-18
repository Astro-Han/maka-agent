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
    Epoch, GraphId, Mode,
    control::{Control, EpochPage},
};
use sqlx::Connection;

impl EventLog {
    pub async fn graph_roots(&self, after: Option<&str>) -> Result<Vec<String>, StoreError> {
        self.validate_root()?;
        if let Some(after) = after {
            crate::sessions::validate_id(after)?;
        }
        let after = after.map(str::to_owned);
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    Ok(sqlx::query_scalar(
                        "SELECT DISTINCT root_session_id FROM graph_epochs
                 WHERE (?1 IS NULL OR root_session_id > ?1) ORDER BY root_session_id LIMIT 64",
                    )
                    .bind(after)
                    .fetch_all(connection)
                    .await?)
                })
            })
            .await
    }

    pub async fn graph_control(
        &self,
        root: &str,
        graph: Option<&GraphId>,
    ) -> Result<Option<Control>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(root)?;
        let root = root.to_owned();
        let graph = graph.cloned();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let graph = match graph {
                        Some(graph) => Some(graph),
                        None => current(&mut tx, &root).await?,
                    };
                    let Some(graph) = graph else {
                        return Ok(None);
                    };
                    let control = control(&mut tx, &graph).await?;
                    if control
                        .as_ref()
                        .is_some_and(|row| row.epoch.root_session_id != root)
                    {
                        return Err(invalid("graph belongs to another root Session"));
                    }
                    Ok(control)
                })
            })
            .await
    }

    /// Caller owns Graph coordination and proves the old graph is quiescent.
    /// Closure prevents fresh intents; this CAS prevents an old epoch's rollover
    /// from replacing a newer root binding.
    pub async fn open_graph(
        &self,
        root: &str,
        mode: Mode,
        expected_previous: Option<&GraphId>,
        now: u64,
    ) -> Result<Epoch, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(root)?;
        crate::sessions::validate_time(now)?;
        let root = root.to_owned();
        let expected = expected_previous.cloned();
        let commits = self.commits.clone();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
            let archived: Option<bool> = sqlx::query_scalar("SELECT archived FROM session_control WHERE id = ?")
                .bind(&root).fetch_optional(&mut *tx).await?;
            match archived { Some(false) => {}, Some(true) => return Err(invalid("graph Session is archived")), None => return Err(StoreError::SessionNotFound) }
            let previous = current(&mut tx, &root).await?;
            let epoch = match (previous, expected) {
                (None, None) => 1,
                (Some(id), None) => {
                    let existing = control(&mut tx, &id).await?.ok_or_else(|| invalid("current graph disappeared"))?;
                    if existing.epoch.mode != mode { return Err(invalid("current graph mode differs")); }
                    return Ok(existing.epoch);
                }
                (Some(id), Some(expected)) if id == expected => {
                    let previous = control(&mut tx, &id).await?.ok_or_else(|| invalid("current graph disappeared"))?;
                    if !previous.closed() { return Err(invalid("current graph is not closed")); }
                    previous.epoch.epoch.checked_add(1).filter(|value| *value < (1 << 53)).ok_or_else(|| invalid("graph epoch exhausted"))?
                }
                _ => return Err(StoreError::EventConflict),
            };
            let record = Epoch { root_session_id: root, epoch, graph_id: GraphId::new(), created_at: now, mode };
            sqlx::query("INSERT INTO graph_epochs (root_session_id, epoch, graph_id, mode, created_at) VALUES (?, ?, ?, ?, ?)")
                .bind(&record.root_session_id).bind(record.epoch as i64).bind(record.graph_id.as_str())
                .bind(match mode { Mode::Graph => "graph", Mode::Swarm => "swarm" }).bind(now as i64)
                .execute(&mut *tx).await?;
            tx.commit().await.map_err(StoreError::CommitUnknown)?;
            commits.send_modify(|_| {});
            Ok(record)
        })).await
    }

    pub async fn graph_epochs(
        &self,
        root: &str,
        before: Option<u64>,
    ) -> Result<EpochPage, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(root)?;
        if before.is_some_and(|value| value == 0 || value > (1 << 53) - 1) {
            return Err(invalid("invalid epoch cursor"));
        }
        let root = root.to_owned();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin().await?;
            let current: Option<i64> = sqlx::query_scalar("SELECT MAX(epoch) FROM graph_epochs WHERE root_session_id = ?")
                .bind(&root).fetch_one(&mut *tx).await?;
            let rows: Vec<(String, i64, String, i64)> = sqlx::query_as(
                "SELECT graph_id, epoch, mode, created_at FROM graph_epochs WHERE root_session_id = ?1
                 AND (?2 IS NULL OR epoch < ?2) ORDER BY epoch DESC LIMIT 33"
            ).bind(&root).bind(before.map(|n| n as i64)).fetch_all(&mut *tx).await?;
            let more = rows.len() > 32;
            let epochs = rows.into_iter().take(32).map(|(id, epoch, mode, created)| {
                let epoch = Epoch { root_session_id: root.clone(), graph_id: id.try_into().map_err(invalid)?,
                    epoch: u64::try_from(epoch).map_err(invalid)?, created_at: u64::try_from(created).map_err(invalid)?,
                    mode: match mode.as_str() { "graph" => Mode::Graph, "swarm" => Mode::Swarm, _ => return Err(invalid("invalid graph mode")) }
                };
                epoch.validate().map_err(invalid)?;
                Ok(epoch)
            }).collect::<Result<Vec<_>, StoreError>>()?;
            let next_before = more.then(|| epochs.last().expect("nonempty overflow page").epoch);
            Ok(EpochPage { epochs, next_before, current_epoch: current.map(u64::try_from).transpose().map_err(invalid)? })
        })).await
    }

    pub async fn stop_graph(&self, root: &str, expected: &GraphId) -> Result<Control, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(root)?;
        let root = root.to_owned();
        let expected = expected.clone();
        let commits = self.commits.clone();
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
                    if current(&mut tx, &root).await?.as_ref() != Some(&expected) {
                        return Err(StoreError::EventConflict);
                    }
                    sqlx::query("UPDATE graph_epochs SET stop_requested = 1 WHERE graph_id = ?")
                        .bind(expected.as_str())
                        .execute(&mut *tx)
                        .await?;
                    let result = control(&mut tx, &expected)
                        .await?
                        .ok_or_else(|| invalid("current graph disappeared"))?;
                    tx.commit().await.map_err(StoreError::CommitUnknown)?;
                    commits.send_modify(|_| {});
                    Ok(result)
                })
            })
            .await
    }
}
