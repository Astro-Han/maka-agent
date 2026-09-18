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

use crate::{Epoch, Error, GraphId, OperatorId, WorkId, identity};
use maka_runtime::event::Invocation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Target {
    Agent {
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        executor_id: Option<String>,
    },
    Preset {
        preset_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        executor_id: Option<String>,
    },
    Operator {
        operator_id: OperatorId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoricalInput {
    pub source_graph_id: GraphId,
    pub result_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Work {
    pub work_id: WorkId,
    pub target: Target,
    pub instruction: String,
    pub input_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_result_inputs: Vec<HistoricalInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Stop {
    pub target_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Finish {
    pub result_ids: Vec<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub invocation: Invocation,
    pub operation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Update {
    pub graph_id: GraphId,
    pub source: Source,
    pub add_work: Vec<Work>,
    pub stop: Vec<Stop>,
    pub finish: Option<Finish>,
}

impl Update {
    pub fn validate(&self) -> Result<(), Error> {
        for id in [
            &self.source.invocation.session_id,
            &self.source.invocation.turn_id,
            &self.source.invocation.run_id,
            &self.source.invocation.invocation_id,
            &self.source.operation_id,
        ] {
            identity(id)?;
        }
        if self.add_work.len() > 32
            || self.stop.len() > 20
            || (self.add_work.is_empty() && self.stop.is_empty() && self.finish.is_none())
            || (self.finish.is_some() && !self.add_work.is_empty())
        {
            return Err(Error::Invalid("invalid schedule update cardinality".into()));
        }
        let mut work_ids = BTreeSet::new();
        let mut selected = 0;
        for work in &self.add_work {
            if !work_ids.insert(&work.work_id) {
                return Err(Error::Invalid("duplicate work".into()));
            }
            bounded_text(&work.instruction, 60_000)?;
            match &work.target {
                Target::Agent {
                    agent_id,
                    executor_id,
                } => {
                    identity(agent_id)?;
                    if let Some(id) = executor_id {
                        identity(id)?;
                    }
                }
                Target::Preset {
                    preset_id,
                    executor_id,
                } => {
                    identity(preset_id)?;
                    if let Some(id) = executor_id {
                        identity(id)?;
                    }
                }
                Target::Operator { .. } => {}
            }
            if work.input_ids.len() + work.selected_result_inputs.len() > 64 {
                return Err(Error::Invalid("work input limit exceeded".into()));
            }
            let mut inputs = BTreeSet::new();
            for id in work.input_ids.iter().chain(
                work.selected_result_inputs
                    .iter()
                    .map(|input| &input.result_id),
            ) {
                identity(id)?;
                if !inputs.insert(id) {
                    return Err(Error::Invalid("duplicate work input".into()));
                }
            }
            for historical in &work.selected_result_inputs {
                if historical.source_graph_id == self.graph_id {
                    return Err(Error::Invalid(
                        "historical input references current graph".into(),
                    ));
                }
            }
            selected += work.selected_result_inputs.len();
            if let Some(id) = &work.replaces {
                identity(id)?;
            }
        }
        if selected > 64 {
            return Err(Error::Invalid("historical input limit exceeded".into()));
        }
        let mut stops = BTreeSet::new();
        for stop in &self.stop {
            identity(&stop.target_id)?;
            bounded_text(&stop.reason, 4_000)?;
            if !stops.insert(&stop.target_id) {
                return Err(Error::Invalid("duplicate stop target".into()));
            }
        }
        if let Some(finish) = &self.finish {
            bounded_text(&finish.reason, 4_000)?;
            if finish.result_ids.is_empty() || finish.result_ids.len() > 64 {
                return Err(Error::Invalid("finish requires 1..=64 results".into()));
            }
            let mut results = BTreeSet::new();
            for id in &finish.result_ids {
                identity(id)?;
                if !results.insert(id) {
                    return Err(Error::Invalid("duplicate finish result".into()));
                }
            }
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> Result<String, Error> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| Error::Invalid(error.to_string()))?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    pub fn identity(&self) -> String {
        let bytes = serde_json::to_vec(&self.source).expect("typed source serializes");
        format!("graph_update_{:x}", Sha256::digest(bytes))
    }
}

fn bounded_text(value: &str, characters: usize) -> Result<(), Error> {
    if value.trim().is_empty() || value.chars().count() > characters || value.contains('\0') {
        return Err(Error::Invalid("invalid instruction or reason".into()));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommittedUpdate {
    pub update: Update,
    pub revision: u64,
    pub committed_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Requested,
    Stopped,
    Superseded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledWork {
    pub work: Work,
    pub status: WorkStatus,
    pub revision: u64,
    pub committed_at: u64,
}

/// Derived state only. Committed updates remain the business authority.
#[derive(Clone, Debug)]
pub struct Schedule {
    pub epoch: Epoch,
    pub revision: u64,
    pub work: BTreeMap<WorkId, ScheduledWork>,
    pub stopped: BTreeMap<String, (Stop, u64, u64)>,
    pub finish: Option<(Finish, u64, u64)>,
}

impl Schedule {
    pub fn new(epoch: Epoch) -> Result<Self, Error> {
        epoch.validate()?;
        Ok(Self {
            epoch,
            revision: 0,
            work: BTreeMap::new(),
            stopped: BTreeMap::new(),
            finish: None,
        })
    }

    pub fn apply(&mut self, committed: &CommittedUpdate) -> Result<(), Error> {
        committed.update.validate()?;
        if self.work.len() + committed.update.add_work.len() > 1024 {
            return Err(Error::Invalid("graph epoch exceeds 1024 work items".into()));
        }
        if committed.update.graph_id != self.epoch.graph_id
            || committed.update.source.invocation.session_id != self.epoch.root_session_id
            || committed.committed_at > (1 << 53) - 1
        {
            return Err(Error::Invalid(
                "schedule belongs to a different graph root".into(),
            ));
        }
        if committed.revision != self.revision.checked_add(1).ok_or(Error::Conflict)? {
            return Err(Error::Conflict);
        }
        if self.finish.is_some() {
            return Err(Error::Closed);
        }
        if committed
            .update
            .add_work
            .iter()
            .any(|work| self.work.contains_key(&work.work_id))
        {
            return Err(Error::Conflict);
        }
        // Validation completes before projection changes. Runtime record
        // existence/readiness is resolved separately; missing inputs wait.
        for work in &committed.update.add_work {
            if let Some(replaces) = &work.replaces {
                if let Some(existing) = self
                    .work
                    .values_mut()
                    .find(|item| item.work.work_id.as_str() == replaces)
                {
                    existing.status = WorkStatus::Superseded;
                }
                self.stopped.insert(
                    replaces.clone(),
                    (
                        Stop {
                            target_id: replaces.clone(),
                            reason: format!("Superseded by {}", work.work_id),
                        },
                        committed.revision,
                        committed.committed_at,
                    ),
                );
            }
            self.work.insert(
                work.work_id.clone(),
                ScheduledWork {
                    work: work.clone(),
                    status: WorkStatus::Requested,
                    revision: committed.revision,
                    committed_at: committed.committed_at,
                },
            );
        }
        for stop in &committed.update.stop {
            self.stopped.insert(
                stop.target_id.clone(),
                (stop.clone(), committed.revision, committed.committed_at),
            );
            for work in self.work.values_mut() {
                if work.work.work_id.as_str() == stop.target_id
                    || matches!(&work.work.target, Target::Operator { operator_id } if operator_id.as_str() == stop.target_id)
                {
                    work.status = WorkStatus::Stopped;
                }
            }
        }
        if let Some(finish) = &committed.update.finish {
            self.finish = Some((finish.clone(), committed.revision, committed.committed_at));
        }
        self.revision = committed.revision;
        Ok(())
    }
}
