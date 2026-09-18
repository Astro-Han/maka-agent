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

//! Compact Swarm checkpoints contain status and final-result identities, never child output.
use crate::{GraphId, WorkId, coordinator::Coordinator, view::WorkState};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    NeedsAttention,
    Settled,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub work_id: WorkId,
    pub status: WorkState,
    pub result_record_id: Option<String>,
    pub failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention_id: Option<String>,
}
pub struct Snapshot {
    graph_id: GraphId,
    pub status: Status,
    counts: BTreeMap<WorkState, usize>,
    items: Vec<Item>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<'a> {
    kind: &'static str,
    swarm_id: &'a GraphId,
    status: Status,
    counts: &'a BTreeMap<WorkState, usize>,
    items: &'a [Item],
    next_after: Option<&'a WorkId>,
}
impl Snapshot {
    fn new(graph_id: GraphId, items: Vec<Item>, quiescent: bool) -> Self {
        let mut counts = BTreeMap::new();
        for item in &items {
            *counts.entry(item.status).or_default() += 1;
        }
        let status = if items.iter().any(|item| attention(item.status)) {
            Status::NeedsAttention
        } else if !items.is_empty() && items.iter().all(|item| terminal(item.status)) && quiescent {
            Status::Settled
        } else {
            Status::Running
        };
        Self {
            graph_id,
            status,
            counts,
            items,
        }
    }
    pub fn page(&self, after: Option<&WorkId>) -> Page<'_> {
        let start = after.map_or(0, |id| {
            self.items.partition_point(|item| &item.work_id <= id)
        });
        let end = (start + 32).min(self.items.len());
        Page {
            kind: "agent_swarm_status",
            swarm_id: &self.graph_id,
            status: self.status,
            counts: &self.counts,
            items: &self.items[start..end],
            next_after: (end < self.items.len()).then(|| &self.items[end - 1].work_id),
        }
    }
    pub(crate) fn signal_key(&self) -> Option<String> {
        if self.status == Status::Running {
            return None;
        }
        // While attention is outstanding, unrelated successful siblings do not
        // repeatedly wake the supervisor. Replacements remove retired failures.
        let items = self
            .items
            .iter()
            .filter(|item| self.status == Status::Settled || attention(item.status))
            .collect::<Vec<_>>();
        let bytes =
            serde_json::to_vec(&(&self.graph_id, self.status, items)).expect("typed Swarm signal");
        Some(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}
impl Coordinator {
    pub(crate) fn swarm_snapshot(&self) -> Option<Snapshot> {
        if self.schedule.epoch.mode != crate::Mode::Swarm {
            return None;
        }
        let items = self
            .schedule
            .work
            .values()
            .map(|work| {
                let id = &work.work.work_id;
                Item {
                    work_id: id.clone(),
                    status: self.work_state(work),
                    attention_id: self
                        .executions
                        .get(id)
                        .and_then(|execution| execution.observation.attention_id.clone()),
                    result_record_id: self
                        .executions
                        .get(id)
                        .filter(|execution| execution.settled())
                        .and_then(|execution| {
                            execution
                                .terminal
                                .as_ref()
                                .map(|record| record.record_id.clone())
                        }),
                    failure: self.failures.get(id).map(|reason| {
                        reason[..reason.floor_char_boundary(reason.len().min(512))].into()
                    }),
                }
            })
            .collect::<Vec<_>>();
        Some(Snapshot::new(
            self.schedule.epoch.graph_id.clone(),
            items,
            self.quiescent() && !self.catching_up(),
        ))
    }
}
fn attention(state: WorkState) -> bool {
    matches!(
        state,
        WorkState::Blocked | WorkState::Failed | WorkState::Cancelled
    )
}
fn terminal(state: WorkState) -> bool {
    matches!(
        state,
        WorkState::Completed
            | WorkState::Failed
            | WorkState::Cancelled
            | WorkState::Stopped
            | WorkState::Superseded
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_is_actionable_not_sibling_chatter_and_paging_preserves_the_tail() {
        let graph = GraphId::new();
        let snapshot = |attention_state, sibling_state, settled| {
            Snapshot::new(
                graph.clone(),
                (0..34)
                    .map(|index| Item {
                        work_id: format!("graph_work_{index:02}").try_into().unwrap(),
                        status: if index == 33 {
                            attention_state
                        } else {
                            sibling_state
                        },
                        result_record_id: None,
                        failure: None,
                        attention_id: (index == 33).then(|| "question-1".into()),
                    })
                    .collect(),
                settled,
            )
        };
        let blocked = snapshot(WorkState::Blocked, WorkState::Running, false);
        assert_eq!(
            blocked.status,
            Status::NeedsAttention,
            "off-page blocked work is actionable"
        );
        let completed_siblings = snapshot(WorkState::Blocked, WorkState::Completed, false);
        assert_eq!(blocked.signal_key(), completed_siblings.signal_key());
        let mut next_question = snapshot(WorkState::Blocked, WorkState::Completed, false);
        next_question.items[33].attention_id = Some("question-2".into());
        assert_ne!(blocked.signal_key(), next_question.signal_key());
        let replaced = snapshot(WorkState::Superseded, WorkState::Running, false);
        assert_eq!(
            replaced.signal_key(),
            None,
            "replaced failure is no longer actionable"
        );
        let draining = snapshot(WorkState::Superseded, WorkState::Completed, false);
        assert_eq!(
            draining.status,
            Status::Running,
            "terminal facts do not prove resource cleanup"
        );
        let settled = snapshot(WorkState::Superseded, WorkState::Completed, true);
        assert_eq!(settled.status, Status::Settled);
        assert_ne!(settled.signal_key(), blocked.signal_key());
        let first = blocked.page(None);
        assert_eq!(first.items.len(), 32);
        let last = blocked.page(first.next_after);
        assert_eq!(last.items.len(), 2);
        assert_eq!(last.items[1].status, WorkState::Blocked);
        assert!(last.next_after.is_none());
        assert_eq!(last.counts[&WorkState::Running], 33);
    }
}
