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

use crate::{EventLog, StoreError};
use sqlx::{Connection, Row};

/// Execution evidence for Sessions sharing an exact canonical working directory.
/// No workspace registry: membership is projected from durable Session bindings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceFence(pub Vec<WorkspaceSession>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSession {
    pub session_id: String,
    pub revision: i64,
    pub event_sequence: i64,
    pub orphaned_shells: bool,
}

impl EventLog {
    pub async fn workspace_fence(&self, cwd: &str) -> Result<WorkspaceFence, StoreError> {
        self.validate_root()?;
        let cwd = cwd.to_owned();
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin().await?;
            let rows = sqlx::query(
                "SELECT s.id, s.revision,
                 COALESCE((SELECT MAX(e.sequence) FROM runtime_events e WHERE
                    json_extract(e.event_json, '$.invocation.session_id') = s.id), 0) AS sequence,
                 EXISTS(SELECT 1 FROM shell_runs r WHERE r.session_id = s.id AND
                    json_extract(r.record_json, '$.state.outcome.kind') = 'orphaned') AS orphaned
                 FROM session_control s WHERE json_extract(s.configuration, '$.workspace.hostCwd') = ?
                 ORDER BY s.id LIMIT 257"
            ).bind(cwd).fetch_all(&mut *tx).await?;
            if rows.len() > 256 { return Err(StoreError::PrefixTooLarge); }
            let mut result = Vec::with_capacity(rows.len());
            for row in rows {
                result.push(WorkspaceSession {
                    session_id: row.try_get("id")?, revision: row.try_get("revision")?,
                    event_sequence: row.try_get("sequence")?, orphaned_shells: row.try_get("orphaned")?,
                });
            }
            tx.commit().await?;
            Ok(WorkspaceFence(result))
        })).await
    }
}
