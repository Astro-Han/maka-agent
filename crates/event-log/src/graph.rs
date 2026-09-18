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

//! Durable Graph control facts. Runtime execution remains Host-owned.
mod epochs;
mod intents;
mod store;
mod updates;
mod wakes;

use crate::StoreError;
use maka_graph::{Epoch, GraphId, Mode, control::Control};
use sqlx::SqliteConnection;

fn invalid(reason: impl ToString) -> StoreError {
    StoreError::InvalidTransition(reason.to_string())
}

async fn control(
    connection: &mut SqliteConnection,
    id: &GraphId,
) -> Result<Option<Control>, StoreError> {
    let row: Option<(String, i64, String, i64, bool)> = sqlx::query_as(
        "SELECT root_session_id, epoch, mode, created_at, stop_requested FROM graph_epochs WHERE graph_id = ?"
    ).bind(id.as_str()).fetch_optional(&mut *connection).await?;
    let Some((root, epoch, mode, created, stopped)) = row else {
        return Ok(None);
    };
    let epoch = Epoch {
        root_session_id: root,
        epoch: u64::try_from(epoch).map_err(invalid)?,
        graph_id: id.clone(),
        mode: match mode.as_str() {
            "graph" => Mode::Graph,
            "swarm" => Mode::Swarm,
            _ => return Err(invalid("invalid stored graph mode")),
        },
        created_at: u64::try_from(created).map_err(invalid)?,
    };
    epoch.validate().map_err(invalid)?;
    let (revision, finished): (i64, bool) = sqlx::query_as(
        "SELECT COALESCE(MAX(revision), 0), COALESCE(MAX(json_type(update_json, '$.finish') = 'object'), 0)
         FROM graph_updates WHERE graph_id = ?"
    ).bind(id.as_str()).fetch_one(connection).await?;
    Ok(Some(Control {
        epoch,
        schedule_revision: u64::try_from(revision).map_err(invalid)?,
        stop_requested: stopped,
        finished,
    }))
}

async fn current(
    connection: &mut SqliteConnection,
    root: &str,
) -> Result<Option<GraphId>, StoreError> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT graph_id FROM graph_epochs WHERE root_session_id = ? ORDER BY epoch DESC LIMIT 1",
    )
    .bind(root)
    .fetch_optional(connection)
    .await?;
    id.map(|id| id.try_into().map_err(invalid)).transpose()
}
