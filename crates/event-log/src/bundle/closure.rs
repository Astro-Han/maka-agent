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

use super::{Inventory, format::MAX_EVENT_BYTES, format::MAX_EVENTS};
use crate::{StoreError, context::selection::Selection, sequence_number};
use maka_runtime::{
    context::CheckpointMode,
    event::{Fact, LogScope, RuntimeEvent},
};
use sqlx::SqliteConnection;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

/// Event identities and exact copied memberships, not additional catalog Sessions.
pub(super) struct Closure {
    pub events: BTreeSet<u64>,
    pub members: BTreeMap<(String, u64), Option<u64>>,
    pub copies: BTreeSet<String>,
    pub revisions: BTreeSet<(String, u64)>,
    queue: VecDeque<u64>,
    scopes: HashMap<String, u64>,
    runs: HashMap<String, u64>,
    bytes: u64,
}

impl Closure {
    pub async fn capture(
        db: &mut SqliteConnection,
        inventory: &Inventory,
        through: u64,
    ) -> Result<Self, StoreError> {
        let mut selected = Self {
            events: BTreeSet::new(),
            members: BTreeMap::new(),
            copies: BTreeSet::new(),
            revisions: BTreeSet::new(),
            queue: VecDeque::new(),
            scopes: HashMap::new(),
            runs: HashMap::new(),
            bytes: 0,
        };
        for session in &inventory.sessions {
            let busy: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM local_runtime_events o WHERE o.kind='invocation_opened' AND o.event_session=?1
                 AND NOT EXISTS(SELECT 1 FROM runtime_events t WHERE t.invocation_id=o.invocation_id AND t.kind='invocation_ended'))
                 OR EXISTS(SELECT 1 FROM message_admissions WHERE session_id=?1)
                 OR EXISTS(SELECT 1 FROM session_processes WHERE session_id=?1 AND cleaned=0)"
            ).bind(&session.id).fetch_one(&mut *db).await?;
            if busy {
                return Err(StoreError::SessionBusy);
            }
            selected
                .scope(
                    db,
                    &LogScope::Session {
                        id: session.id.clone(),
                    },
                    through,
                )
                .await?;
            let revisions: Vec<i64> = sqlx::query_scalar(
                "SELECT sequence FROM session_revision_sources WHERE session_id=? ORDER BY sequence LIMIT ?"
            ).bind(&session.id).bind((MAX_EVENTS+1) as i64).fetch_all(&mut *db).await?;
            for sequence in revisions {
                let sequence = sequence_number(sequence)?;
                selected.add(sequence)?;
                selected.revisions.insert((session.id.clone(), sequence));
                if selected.revisions.len() > MAX_EVENTS {
                    return Err(StoreError::PrefixTooLarge);
                }
            }
        }
        while let Some(sequence) = selected.queue.pop_front() {
            let row: Option<(Option<String>, Option<i64>)> = sqlx::query_as(
                "SELECT CASE WHEN length(CAST(event_json AS BLOB))<=?2 THEN event_json END,
                    (SELECT length(payload) FROM tool_result_payloads WHERE event_id=e.event_id)
                 FROM runtime_events e WHERE sequence=?1",
            )
            .bind(sequence as i64)
            .bind(MAX_EVENT_BYTES as i64)
            .fetch_optional(&mut *db)
            .await?;
            let (json, payload_bytes) = row.ok_or(StoreError::MaterialCollected)?;
            let json = json.ok_or(StoreError::PrefixTooLarge)?;
            selected.bytes +=
                json.len() as u64 + payload_bytes.map(sequence_number).transpose()?.unwrap_or(0);
            if selected.bytes > super::format::MAX_BYTES {
                return Err(StoreError::PrefixTooLarge);
            }
            let event: RuntimeEvent = serde_json::from_str(&json)?;
            crate::tool_payloads::verify_binding(&event, payload_bytes)?;
            if !matches!(event.fact, Fact::MessageImported { .. }) {
                selected
                    .run(db, &event.invocation.invocation_id, sequence)
                    .await?;
            }
            match &event.fact {
                Fact::ModelRequested {
                    source_scope,
                    source_high_water,
                    ..
                } => {
                    selected.scope(db, source_scope, *source_high_water).await?;
                    selected
                        .archives(db, source_scope, *source_high_water, sequence)
                        .await?;
                }
                Fact::ContextCheckpointRecorded { checkpoint } => {
                    let selection = Selection::for_invocation(db, &event.invocation).await?;
                    selected
                        .scope(db, &selection.scope, checkpoint.covered_through)
                        .await?;
                    if matches!(checkpoint.mode, CheckpointMode::Standalone) {
                        let terminal: i64 = sqlx::query_scalar(
                            "SELECT sequence FROM runtime_events WHERE invocation_id=? AND kind='invocation_ended'"
                        ).bind(&event.invocation.invocation_id).fetch_one(&mut *db).await?;
                        selected.add(sequence_number(terminal)?)?;
                    }
                }
                Fact::InvocationOpened { input, .. } => {
                    if let Some(claim) = input.inherited_claim() {
                        let source = &claim.source.invocation;
                        let end: i64 = sqlx::query_scalar(
                            "SELECT sequence FROM runtime_events WHERE invocation_id=? AND kind='invocation_ended'"
                        ).bind(&source.invocation_id).fetch_one(&mut *db).await?;
                        selected
                            .run(db, &source.invocation_id, sequence_number(end)?)
                            .await?;
                        selected
                            .scope(
                                db,
                                &LogScope::Session {
                                    id: source.session_id.clone(),
                                },
                                claim.base.high_water,
                            )
                            .await?;
                    }
                }
                Fact::ToolResultArchived { placeholder } => {
                    let target: i64 =
                        sqlx::query_scalar("SELECT sequence FROM runtime_events WHERE event_id=?")
                            .bind(&placeholder.identity.runtime_event_id)
                            .fetch_one(&mut *db)
                            .await?;
                    let selection = Selection::for_invocation(db, &event.invocation).await?;
                    selected
                        .scope(db, &selection.scope, sequence_number(target)?)
                        .await?;
                }
                _ => {}
            }
        }
        Ok(selected)
    }

    fn add(&mut self, sequence: u64) -> Result<(), StoreError> {
        if self.events.insert(sequence) {
            if self.events.len() > MAX_EVENTS {
                return Err(StoreError::PrefixTooLarge);
            }
            self.queue.push_back(sequence);
        }
        Ok(())
    }

    async fn run(
        &mut self,
        db: &mut SqliteConnection,
        invocation: &str,
        through: u64,
    ) -> Result<(), StoreError> {
        let prior = self.runs.get(invocation).copied().unwrap_or(0);
        if through <= prior {
            return Ok(());
        }
        let rows: Vec<i64> = sqlx::query_scalar(
            "SELECT sequence FROM runtime_events WHERE invocation_id=? AND sequence>? AND sequence<=? ORDER BY sequence LIMIT ?"
        ).bind(invocation).bind(prior as i64).bind(through as i64).bind((MAX_EVENTS+1) as i64).fetch_all(db).await?;
        for row in rows {
            self.add(sequence_number(row)?)?;
        }
        self.runs.insert(invocation.into(), through);
        Ok(())
    }

    async fn scope(
        &mut self,
        db: &mut SqliteConnection,
        scope: &LogScope,
        through: u64,
    ) -> Result<(), StoreError> {
        if through > i64::MAX as u64 {
            return Err(StoreError::PrefixTooLarge);
        }
        if matches!(scope, LogScope::Root) {
            return Err(StoreError::InvalidTransition(
                "Root-scoped proof cannot be exported as a Session bundle".into(),
            ));
        }
        let key = serde_json::to_string(scope)?;
        let prior = self.scopes.get(&key).copied();
        if prior.is_some_and(|previous| through <= previous) {
            return Ok(());
        }
        let selection = Selection::resolve(db, scope).await?;
        let session = selection.session.as_ref().expect("non-root selection");
        let copy: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM session_history_copies WHERE session_id=?)",
        )
        .bind(session)
        .fetch_one(&mut *db)
        .await?;
        if copy {
            self.copies.insert(session.clone());
            if self.copies.len() > super::MAX_SESSIONS {
                return Err(StoreError::PrefixTooLarge);
            }
        }
        let filter = Selection::predicate("e", "?3");
        let collected: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT EXISTS(SELECT 1 FROM event_log e WHERE e.event_session=?1 AND e.invocation_id IS NOT NULL
               AND e.sequence<=?2 AND e.event_json IS NULL AND {filter}
             UNION ALL SELECT 1 FROM session_history_members h JOIN event_log e ON e.sequence=h.sequence
             WHERE h.session_id=?1 AND e.sequence<=?2 AND e.event_json IS NULL AND {filter})"
        ))).bind(session).bind(through as i64).bind(&selection.lineage).fetch_one(&mut *db).await?;
        if collected {
            return Err(StoreError::MaterialCollected);
        }
        let filter = Selection::predicate("e", "?4");
        let rows: Vec<(i64, bool, Option<i64>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT e.sequence,e.inherited,e.archive_sequence FROM session_history_events e
             WHERE e.owner_session_id=?1 AND e.sequence>?2 AND e.sequence<=?3 AND {filter}
             ORDER BY e.sequence LIMIT ?5"
        )))
        .bind(session)
        .bind(prior.unwrap_or(0) as i64)
        .bind(through as i64)
        .bind(&selection.lineage)
        .bind((MAX_EVENTS + 1) as i64)
        .fetch_all(&mut *db)
        .await?;
        for (sequence, inherited, archive) in rows {
            let sequence = sequence_number(sequence)?;
            self.add(sequence)?;
            if inherited {
                let archive = archive.map(sequence_number).transpose()?;
                if let Some(archive) = archive {
                    self.add(archive)?;
                }
                self.members.insert((session.clone(), sequence), archive);
                if self.members.len() > MAX_EVENTS {
                    return Err(StoreError::PrefixTooLarge);
                }
            }
        }
        self.scopes.insert(key, through);
        Ok(())
    }

    async fn archives(
        &mut self,
        db: &mut SqliteConnection,
        scope: &LogScope,
        through: u64,
        before: u64,
    ) -> Result<(), StoreError> {
        let selection = Selection::resolve(db, scope).await?;
        let filter = Selection::archive_predicate("t", "a", "?3", "?4");
        let target = Selection::predicate("t", "?4");
        let rows: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT a.sequence FROM session_history_events t JOIN runtime_events a
             ON a.kind='tool_result_archived' AND json_extract(a.event_json,'$.fact.placeholder.identity.runtime_event_id')=t.event_id
             WHERE t.owner_session_id=?1 AND t.sequence<=?2 AND {filter} AND {target}
             ORDER BY a.sequence LIMIT ?5"
        ))).bind(&selection.session).bind(through as i64).bind(before as i64).bind(&selection.lineage)
            .bind((MAX_EVENTS+1) as i64).fetch_all(db).await?;
        for row in rows {
            self.add(sequence_number(row)?)?;
        }
        Ok(())
    }
}
