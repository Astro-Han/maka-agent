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

use super::{ArchiveError, target};
use crate::{
    EventLog, StoreError,
    context::{ArchivedToolResult, read, selection::Selection},
    sequence_number,
};
use maka_runtime::{
    archive::{ArchiveIdentity, ArchivedPlaceholder, encode_projection, projection_digest},
    event::{Fact, RuntimeEvent},
};
use sha2::{Digest, Sha256};
use sqlx::{Connection, SqliteConnection};

pub(super) fn validate(
    target: &target::Target,
    placeholder: &ArchivedPlaceholder,
) -> Result<Vec<u8>, StoreError> {
    placeholder.validate().map_err(|_| ArchiveError::Corrupt)?;
    target
        .projection
        .validate(&target.invocation.session_id)
        .map_err(|_| ArchiveError::Corrupt)?;
    let identity = &placeholder.identity;
    if identity.runtime_event_id != target.event_id
        || identity.tool_call_id != target.call.tool_call_id
        || identity.tool_name != target.name
    {
        return Err(ArchiveError::SourceMismatch.into());
    }
    let body = encode_projection(&target.projection).map_err(|_| ArchiveError::Corrupt)?;
    if body.len() as u64 != identity.original_bytes {
        return Err(ArchiveError::SizeMismatch.into());
    }
    if projection_digest(&target.projection).map_err(|_| ArchiveError::Corrupt)?
        != identity.source_projection_digest
        || format!("{:x}", Sha256::digest(&body)) != identity.body_sha256
    {
        return Err(ArchiveError::Corrupt.into());
    }
    let expected = ArchivedPlaceholder::prepare(
        target.event_id.clone(),
        target.call.tool_call_id.clone(),
        target.name.clone(),
        &target.projection,
    )
    .map_err(|_| ArchiveError::Corrupt)?
    .ok_or(ArchiveError::Corrupt)?;
    if expected != *placeholder {
        return Err(ArchiveError::Corrupt.into());
    }
    Ok(body)
}

pub(crate) async fn validate_append(
    connection: &mut SqliteConnection,
    event: &RuntimeEvent,
) -> Result<(), StoreError> {
    let Fact::ToolResultArchived { placeholder } = &event.fact else {
        return Ok(());
    };
    let target = target::read(
        connection,
        &event.invocation.session_id,
        &placeholder.identity.runtime_event_id,
    )
    .await?
    .ok_or(ArchiveError::SourceMismatch)?;
    validate(&target, placeholder)?;
    let previous: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE kind = 'tool_result_archived'
         AND json_extract(event_json, '$.fact.placeholder.identity.runtime_event_id') = ?)",
    )
    .bind(&target.event_id)
    .fetch_one(&mut *connection)
    .await?;
    if previous {
        return Err(StoreError::EventConflict);
    }
    let selection = Selection::for_invocation(connection, &event.invocation).await?;
    let baseline = read::latest_selected(connection, &selection, i64::MAX as u64).await?;
    if baseline
        .as_ref()
        .is_some_and(|b| target.sequence <= b.checkpoint.covered_through)
    {
        return Err(StoreError::InvalidTransition(
            "archive target is already covered by a checkpoint".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn verify_replay(
    connection: &mut SqliteConnection,
    event: &RuntimeEvent,
) -> Result<(), StoreError> {
    if let Fact::ToolResultArchived { placeholder } = &event.fact {
        let (_, accepted) = find(
            connection,
            &event.invocation.session_id,
            &placeholder.identity.runtime_event_id,
            i64::MAX as u64,
        )
        .await?
        .ok_or(ArchiveError::Corrupt)?;
        if accepted != *placeholder {
            return Err(StoreError::EventConflict);
        }
    }
    Ok(())
}

pub(crate) async fn find(
    connection: &mut SqliteConnection,
    session: &str,
    target_id: &str,
    before: u64,
) -> Result<Option<(u64, ArchivedPlaceholder)>, StoreError> {
    let rows: Vec<(i64, Option<String>,String,String)> = sqlx::query_as(
        "SELECT sequence, CASE WHEN length(CAST(event_json AS BLOB)) <= 32768 THEN event_json END,event_id,invocation_id
         FROM runtime_events WHERE kind = 'tool_result_archived'
         AND json_extract(event_json, '$.fact.placeholder.identity.runtime_event_id') = ? AND sequence < ? LIMIT 2",
    ).bind(target_id).bind(before as i64).fetch_all(&mut *connection).await?;
    if rows.len() > 1 {
        return Err(ArchiveError::Corrupt.into());
    }
    let Some((sequence, json, event_id, invocation_id)) = rows.into_iter().next() else {
        return Ok(None);
    };
    let event: RuntimeEvent = serde_json::from_str(&json.ok_or(ArchiveError::Corrupt)?)
        .map_err(|_| ArchiveError::Corrupt)?;
    if event.invocation.session_id != session
        || event.id != event_id
        || event.invocation.invocation_id != invocation_id
    {
        return Err(ArchiveError::Corrupt.into());
    }
    let writer:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_events o WHERE o.invocation_id=? AND o.kind='invocation_opened' AND o.sequence<? AND json_extract(o.event_json,'$.invocation')=json(?) AND NOT EXISTS(SELECT 1 FROM runtime_events t WHERE t.invocation_id=o.invocation_id AND t.kind='invocation_ended' AND t.sequence<?))")
        .bind(&invocation_id).bind(sequence).bind(serde_json::to_string(&event.invocation)?).bind(sequence).fetch_one(&mut *connection).await?;
    if !writer {
        return Err(ArchiveError::Corrupt.into());
    }
    let Fact::ToolResultArchived { placeholder } = event.fact else {
        return Err(ArchiveError::Corrupt.into());
    };
    let target = target::read(connection, session, target_id)
        .await?
        .ok_or(ArchiveError::Corrupt)?;
    if target.sequence >= sequence_number(sequence)? {
        return Err(ArchiveError::Corrupt.into());
    }
    let selection = Selection::for_invocation(connection, &event.invocation).await?;
    // Do not recurse into checkpoint proofs: those validate their archive source here.
    let baseline =
        read::latest_record(connection, &selection, sequence_number(sequence)? - 1).await?;
    let covered = match baseline.map(|b| b.event.fact) {
        Some(Fact::ContextCheckpointRecorded { checkpoint }) => checkpoint.covered_through,
        None => 0,
        _ => return Err(ArchiveError::Corrupt.into()),
    };
    if target.sequence <= covered {
        return Err(ArchiveError::Corrupt.into());
    }
    validate(&target, &placeholder)?;
    Ok(Some((sequence as u64, placeholder)))
}

pub(crate) async fn archived(
    connection: &mut SqliteConnection,
    session: &str,
    target_id: &str,
    before: u64,
) -> Result<Option<ArchivedToolResult>, StoreError> {
    let Some((_, placeholder)) = find(connection, session, target_id, before).await? else {
        return Ok(None);
    };
    // Validation above releases its bounded base; only compact identity survives.
    let (sequence, invocation, operation_id, is_error): (i64,String,String,bool) = sqlx::query_as(
        "SELECT sequence,json_extract(event_json,'$.invocation'),operation_id,
         json_extract(event_json,'$.fact.outcome.kind') = 'failed' FROM runtime_events WHERE event_id = ?",
    ).bind(target_id).fetch_one(connection).await?;
    Ok(Some(ArchivedToolResult {
        sequence: sequence_number(sequence)?,
        event_id: target_id.into(),
        invocation: serde_json::from_str(&invocation)?,
        operation_id,
        replacement: placeholder
            .to_model_projection()
            .map_err(|_| ArchiveError::Corrupt)?,
        is_error,
    }))
}

impl EventLog {
    /// Read the committed model projection with or without a later archive replacement.
    pub async fn read_tool_result(
        &self,
        session: &str,
        event_id: &str,
    ) -> Result<Option<super::ToolResultResource>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session)?;
        let (session, event_id) = (session.to_owned(), event_id.to_owned());
        self.connection
            .run(move |connection| {
                Box::pin(async move {
                    let mut tx = connection.begin().await?;
                    let Some(target) = target::read(&mut tx, &session, &event_id).await? else {
                        return Ok(None);
                    };
                    target
                        .projection
                        .validate(&session)
                        .map_err(|_| ArchiveError::Corrupt)?;
                    let bytes = match find(&mut tx, &session, &event_id, i64::MAX as u64).await? {
                        Some((_, placeholder)) => validate(&target, &placeholder)?,
                        None => maka_runtime::archive::encode_projection(&target.projection)
                            .map_err(|_| ArchiveError::Corrupt)?,
                    };
                    let serialized_result =
                        String::from_utf8(bytes).map_err(|_| ArchiveError::Corrupt)?;
                    tx.commit().await?;
                    Ok(Some(super::ToolResultResource {
                        tool_name: target.name,
                        serialized_result,
                    }))
                })
            })
            .await
    }

    pub async fn read_archive(
        &self,
        session: &str,
        identity: &ArchiveIdentity,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        identity
            .validate()
            .map_err(|_| ArchiveError::SourceMismatch)?;
        let Some((accepted, bytes)) = self
            .read_archive_by_event(session, &identity.runtime_event_id)
            .await?
        else {
            return Ok(None);
        };
        if accepted != *identity {
            return Err(ArchiveError::SourceMismatch.into());
        }
        Ok(Some(bytes))
    }

    /// Resolve a locator through accepted evidence in the caller's Session, never raw payload alone.
    pub async fn read_archive_by_event(
        &self,
        session: &str,
        event_id: &str,
    ) -> Result<Option<(ArchiveIdentity, Vec<u8>)>, StoreError> {
        self.validate_root()?;
        crate::sessions::validate_id(session)?;
        let (session, event_id) = (session.to_owned(), event_id.to_owned());
        self.connection.run(move |connection| Box::pin(async move {
            let mut tx = connection.begin().await?;
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE event_id=? AND json_extract(event_json,'$.invocation.session_id')=?)")
                .bind(&event_id).bind(&session).fetch_one(&mut *tx).await?;
            if !exists { return Ok(None); }
            let Some((_, placeholder)) = find(&mut tx, &session, &event_id, i64::MAX as u64).await? else { return Ok(None); };
            let target = target::read(&mut tx, &session, &event_id).await?.ok_or(ArchiveError::Corrupt)?;
            let body = validate(&target, &placeholder)?;
            tx.commit().await?;
            Ok(Some((placeholder.identity, body)))
        })).await
    }
}
