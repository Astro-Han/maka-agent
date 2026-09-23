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

use super::invalid;
use crate::{
    StoreError,
    bundle::{Inventory, closure::Closure, format::Record},
};
use sqlx::SqliteConnection;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) async fn validate(
    staged: &mut SqliteConnection,
    original: &mut SqliteConnection,
    inventory: &Inventory,
    fence: u64,
) -> Result<(), StoreError> {
    let mut parents: BTreeMap<&str, BTreeSet<String>> = inventory
        .sessions
        .iter()
        .map(|s| (s.id.as_str(), BTreeSet::new()))
        .collect();
    let mut seen = BTreeSet::new();
    let mut after = 0i64;
    loop {
        let row: Option<(i64, String)> = sqlx::query_as(
            "SELECT number,record_json FROM frames WHERE kind='session' AND number>? ORDER BY number LIMIT 1"
        ).bind(after).fetch_optional(&mut *staged).await?;
        let Some((number, json)) = row else { break };
        let Record::Session {
            id,
            parent,
            created_at,
            updated_at,
            configuration,
            ..
        } = serde_json::from_str(&json)?
        else {
            unreachable!()
        };
        crate::sessions::validate_id(&id)?;
        crate::sessions::validate_time(created_at)?;
        crate::sessions::validate_time(updated_at)?;
        if serde_json::to_vec(&configuration)?.len() > crate::sessions::MAX_CONFIGURATION_BYTES {
            return Err(invalid("bundle Session configuration exceeds its limit"));
        }
        let edges = parents
            .get_mut(id.as_str())
            .ok_or_else(|| invalid("bundle catalog differs from its inventory"))?;
        if let Some(parent) = parent {
            crate::sessions::validate_id(&parent)?;
            edges.insert(parent);
        }
        seen.insert(id);
        after = number;
    }
    if seen.len() != inventory.sessions.len() {
        return Err(invalid("bundle catalog differs from its inventory"));
    }
    for (id, edges) in &mut parents {
        // Proof-only owners may end at a frozen prefix. Selected Sessions must
        // remain usable: foreign work cannot later be sealed by local recovery.
        let unfinished: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_events o WHERE o.event_session=?
             AND o.kind='invocation_opened' AND NOT EXISTS(SELECT 1 FROM runtime_events t
               WHERE t.invocation_id=o.invocation_id AND t.kind='invocation_ended'))",
        )
        .bind(id)
        .fetch_one(&mut *original)
        .await?;
        if unfinished {
            return Err(invalid("bundle selected Session contains unfinished work"));
        }
        let missing_input: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM session_history_copies WHERE session_id=?1
             AND json_extract(request_json,'$.purpose.kind')='revision'
             AND NOT EXISTS(SELECT 1 FROM session_revision_sources WHERE session_id=?1))",
        )
        .bind(id)
        .fetch_one(&mut *original)
        .await?;
        if missing_input {
            return Err(invalid("bundle Revision lacks its editable source input"));
        }
        let parent: Option<String> = sqlx::query_scalar(
            "SELECT source_session_id FROM session_history_copies WHERE session_id=?",
        )
        .bind(id)
        .fetch_optional(&mut *original)
        .await?;
        if let Some(parent) = parent.filter(|p| seen.contains(p)) {
            edges.insert(parent);
        }
    }
    // Two distinct ancestry edges may coexist. Check both without recursively
    // following an input-controlled graph, or adopting any parent authority.
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut pending = BTreeMap::new();
    for (id, edges) in &parents {
        pending.insert(*id, edges.len());
        for parent in edges {
            if !seen.contains(parent) {
                return Err(invalid("bundle catalog parent is outside its inventory"));
            }
            children.entry(parent).or_default().push(id);
        }
    }
    let roots: Vec<_> = pending
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| *id)
        .collect();
    if roots != [inventory.root_session_id.as_str()] {
        return Err(invalid("bundle catalog is not the selected rooted subtree"));
    }
    let mut ready = VecDeque::from(roots);
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        for child in children.get(id).into_iter().flatten() {
            let count = pending.get_mut(child).expect("catalog child");
            *count -= 1;
            if *count == 0 {
                ready.push_back(child);
            }
        }
    }
    if visited != seen.len() {
        return Err(invalid("bundle catalog contains an ancestry cycle"));
    }

    // Reuse the export dependency definition. Closure only visits staged rows,
    // so matching counts proves no unrelated original facts or copy links remain.
    let closure = Closure::capture(original, inventory, fence).await?;
    let actual: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM runtime_events),
                (SELECT COUNT(*) FROM session_history_copies),
                (SELECT COUNT(*) FROM session_history_members),
                (SELECT COUNT(*) FROM session_revision_sources)",
    )
    .fetch_one(original)
    .await?;
    if actual
        != (
            closure.events.len() as i64,
            closure.copies.len() as i64,
            closure.members.len() as i64,
            closure.revisions.len() as i64,
        )
    {
        return Err(invalid(
            "bundle contains history outside its selected closure",
        ));
    }
    Ok(())
}
