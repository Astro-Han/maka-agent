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

use crate::StoreError;
use maka_runtime::session_event::SessionEvent;
use sqlx::SqliteConnection;

/// The caller proves the domain transition in this same write transaction.
/// A Session fact has its own position; it never extends an Invocation prefix.
pub(super) async fn append(
    tx: &mut SqliteConnection,
    event: &SessionEvent,
) -> Result<u64, StoreError> {
    event.validate().map_err(super::invalid)?;
    let json = serde_json::to_string(event)?;
    if json.len() > 128 * 1024 {
        return Err(StoreError::PrefixTooLarge);
    }
    let inserted = sqlx::query(
        "INSERT INTO event_log (event_id, invocation_id, kind, operation_id, event_json)
         VALUES (?, NULL, ?, NULL, ?)",
    )
    .bind(&event.id)
    .bind(event.fact.kind())
    .bind(json)
    .execute(tx)
    .await?;
    crate::sequence_number(inserted.last_insert_rowid())
}
