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

use crate::{
    GraphId, OperatorId, WorkId,
    coordinator::Coordinator,
    schedule::{Target, WorkStatus},
};
use maka_plugins::execution::Progress;
use maka_runtime::event::InvocationOutcome;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Empty,
    Active,
    Waiting,
    Closing,
    Stopped,
    Completed,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    Requested,
    Waiting,
    Running,
    Blocked,
    Completed,
    Failed,
    Cancelled,
    Stopped,
    Superseded,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Work {
    pub work_id: WorkId,
    pub operator_id: OperatorId,
    pub target: Target,
    pub status: WorkState,
    pub instruction: String,
    pub instruction_truncated: bool,
    pub waiting_for: Vec<String>,
    pub failure: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_patch: Option<maka_plugins::execution::WorkspacePatch>,
    pub record_id: String,
    pub operator_id: OperatorId,
    pub work_id: WorkId,
    pub text: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub root_session_id: String,
    pub graph_id: GraphId,
    pub revision: u64,
    pub status: Status,
    pub closed: bool,
    pub quiescent: bool,
    pub work: Vec<Work>,
    pub results: Vec<ResultRecord>,
    pub omitted_work: usize,
    pub omitted_results: usize,
}
impl Coordinator {
    pub fn snapshot(&self) -> Snapshot {
        let quiescent = self.quiescent();
        let status = if self.control.closed() {
            if !quiescent {
                Status::Closing
            } else if self.control.finished {
                Status::Completed
            } else {
                Status::Stopped
            }
        } else if !self.failures.is_empty() {
            Status::Failed
        } else if self.schedule.work.is_empty() {
            Status::Empty
        } else if self
            .executions
            .values()
            .any(|execution| !execution.settled())
        {
            Status::Active
        } else {
            Status::Waiting
        };
        let mut work = self.schedule.work.values().collect::<Vec<_>>();
        // Live decisions precede terminal history in a bounded model-facing view.
        work.sort_by_key(|item| {
            (
                self.executions
                    .get(&item.work.work_id)
                    .is_some_and(|execution| execution.settled()),
                std::cmp::Reverse(item.revision),
                item.work.work_id.clone(),
            )
        });
        let work = work
            .into_iter()
            .take(32)
            .map(|item| {
                let id = &item.work.work_id;
                let operator_id = match &item.work.target {
                    Target::Operator { operator_id } => operator_id.clone(),
                    _ => crate::coordinator::operator_id_for(&self.schedule.epoch, id),
                };
                let status = self.work_state(item);
                Work {
                    work_id: id.clone(),
                    operator_id,
                    target: item.work.target.clone(),
                    status,
                    instruction: clip(&item.work.instruction, 512),
                    instruction_truncated: item.work.instruction.len() > 512,
                    waiting_for: self
                        .waiting
                        .get(id)
                        .map(|ids| ids.iter().take(8).cloned().collect())
                        .unwrap_or_default(),
                    failure: self.failures.get(id).map(|reason| clip(reason, 512)),
                }
            })
            .collect::<Vec<_>>();
        let mut records = self
            .executions
            .values()
            .filter(|execution| execution.settled())
            .filter_map(|execution| execution.terminal.as_ref())
            .collect::<Vec<_>>();
        records.sort_by_key(|record| std::cmp::Reverse(record.sequence));
        let result_count = records.len();
        let results = records
            .into_iter()
            .take(16)
            .map(|record| ResultRecord {
                workspace_patch: self.executions.get(&record.work_id).and_then(|execution| {
                    match &execution.workspace_result {
                        crate::projection::WorkspaceResult::Ready(patch) => patch.clone(),
                        crate::projection::WorkspaceResult::Pending => None,
                    }
                }),
                record_id: record.record_id.clone(),
                operator_id: record.operator_id.clone(),
                work_id: record.work_id.clone(),
                text: clip(&record.text, 1024),
                truncated: record.truncated || record.text.len() > 1024,
            })
            .collect::<Vec<_>>();
        Snapshot {
            root_session_id: self.schedule.epoch.root_session_id.clone(),
            graph_id: self.schedule.epoch.graph_id.clone(),
            revision: self.schedule.revision,
            status,
            closed: self.control.closed(),
            quiescent,
            omitted_work: self.schedule.work.len().saturating_sub(work.len()),
            omitted_results: result_count.saturating_sub(results.len()),
            work,
            results,
        }
    }

    pub(crate) fn work_state(&self, item: &crate::schedule::ScheduledWork) -> WorkState {
        match item.status {
            WorkStatus::Stopped => return WorkState::Stopped,
            WorkStatus::Superseded => return WorkState::Superseded,
            WorkStatus::Requested => {}
        }
        let id = &item.work.work_id;
        if self.failures.contains_key(id) {
            return WorkState::Failed;
        }
        match self
            .executions
            .get(id)
            .map(|execution| &execution.observation.progress)
        {
            Some(Progress::Pending) => WorkState::Requested,
            Some(Progress::Running) => WorkState::Running,
            Some(Progress::WaitingForUser | Progress::Paused) => WorkState::Blocked,
            Some(Progress::Ended { outcome }) => match outcome {
                InvocationOutcome::Completed | InvocationOutcome::ContextCompactFinished { .. } => {
                    WorkState::Completed
                }
                InvocationOutcome::Failed { .. } => WorkState::Failed,
                InvocationOutcome::Cancelled { .. } => WorkState::Cancelled,
                InvocationOutcome::HandoffPaused { .. } => WorkState::Blocked,
            },
            None if self.waiting.contains_key(id) => WorkState::Waiting,
            None => WorkState::Requested,
        }
    }

    pub(crate) fn signal_key(&self) -> Option<String> {
        if let Some(swarm) = self.swarm_snapshot() {
            return swarm.signal_key();
        }
        let results = self
            .executions
            .iter()
            .filter(|(_, execution)| execution.settled())
            .filter_map(|(id, execution)| {
                execution
                    .terminal
                    .as_ref()
                    .map(|record| (id, &record.record_id))
            })
            .collect::<Vec<_>>();
        if results.is_empty() && self.failures.is_empty() {
            return None;
        }
        let data = serde_json::to_vec(&(&self.schedule.epoch.graph_id, results, &self.failures))
            .expect("typed signal serializes");
        Some(format!("sha256:{:x}", Sha256::digest(data)))
    }

    /// Includes work outside the bounded presentation page, but not token commits.
    pub(crate) fn change_key(&self) -> String {
        let executions = self
            .executions
            .iter()
            .map(|(id, execution)| {
                (
                    id,
                    &execution.observation.receipt.invocation,
                    &execution.observation.progress,
                    &execution.observation.attention_id,
                    &execution.workspace_result,
                    execution.terminal.as_ref().map(|record| &record.record_id),
                )
            })
            .collect::<Vec<_>>();
        let data = serde_json::to_vec(&(
            &self.schedule.epoch.graph_id,
            self.schedule.revision,
            self.control.closed(),
            executions,
            &self.waiting,
            &self.failures,
        ))
        .expect("typed Graph change surface serializes");
        format!("sha256:{:x}", Sha256::digest(data))
    }
}
fn clip(text: &str, bytes: usize) -> String {
    text[..text.floor_char_boundary(bytes.min(text.len()))].into()
}
