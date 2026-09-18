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

mod detail;
pub(super) mod result;

use maka_event_log::EventLog;
use maka_graph::{
    Epoch, GraphId, WorkId,
    schedule::{Schedule, WorkStatus},
};
use maka_plugins::{composition::Scope, execution::Progress, remote::Error, storage::Namespace};
use maka_runtime::event::InvocationOutcome;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Cursor {
    revision: u64,
    work_id: WorkId,
}
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Query {
    Result {
        graph_id: GraphId,
        work_id: WorkId,
        record_id: String,
        #[serde(default)]
        part: result::Part,
        #[serde(default)]
        offset: usize,
    },
    Work {
        graph_id: GraphId,
        work_id: WorkId,
        #[serde(default)]
        offset: usize,
    },
    Epochs {
        before: Option<u64>,
    },
    Snapshot {
        graph_id: GraphId,
        after: Option<Cursor>,
    },
}
#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum Reply {
    Result {
        result: Option<result::ResultPage>,
    },
    Work {
        work: Option<detail::Detail>,
    },
    Epochs {
        epochs: Vec<Epoch>,
        current_epoch: Option<u64>,
        next_before: Option<u64>,
    },
    Snapshot {
        graph: Option<Graph>,
    },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Graph {
    epoch: Epoch,
    revision: u64,
    stop_requested: bool,
    finished: bool,
    selected_result_ids: Vec<String>,
    work: Vec<Work>,
    total_work: usize,
    next_after: Option<Cursor>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Work {
    work_id: WorkId,
    instruction: String,
    instruction_truncated: bool,
    status: WorkStatus,
    execution: Option<Execution>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Execution {
    session_id: String,
    turn_id: String,
    state: maka_graph::view::WorkState,
    result_record_id: Option<String>,
}

/// Historical reads never activate an epoch, provision a child, or submit work.
pub(super) async fn query(log: &EventLog, root: &str, query: Query) -> Result<Reply, Error> {
    match query {
        Query::Result {
            graph_id,
            work_id,
            record_id,
            part,
            offset,
        } => Ok(Reply::Result {
            result: result::page(log, root, &graph_id, &work_id, &record_id, offset, part).await?,
        }),
        Query::Work {
            graph_id,
            work_id,
            offset,
        } => {
            let work = log
                .graph_work(root, &graph_id, &work_id)
                .await
                .map_err(failure)?;
            Ok(Reply::Work {
                work: work.map(|work| detail::page(work, offset)).transpose()?,
            })
        }
        Query::Epochs { before } => {
            let page = log.graph_epochs(root, before).await.map_err(failure)?;
            Ok(Reply::Epochs {
                epochs: page.epochs,
                current_epoch: page.current_epoch,
                next_before: page.next_before,
            })
        }
        Query::Snapshot { graph_id, after } => {
            let graph = snapshot(log, root, &graph_id, after.as_ref()).await?;
            Ok(Reply::Snapshot { graph })
        }
    }
}
async fn snapshot(
    log: &EventLog,
    root: &str,
    id: &GraphId,
    after: Option<&Cursor>,
) -> Result<Option<Graph>, Error> {
    let Some(control) = log.graph_control(root, Some(id)).await.map_err(failure)? else {
        return Ok(None);
    };
    if after.is_some_and(|cursor| cursor.revision != control.schedule_revision) {
        return Err(Error::Invalid(
            "Graph changed; restart its work pagination".into(),
        ));
    }
    let mut schedule = Schedule::new(control.epoch.clone()).map_err(failure)?;
    let mut truncated = BTreeSet::new();
    while schedule.revision < control.schedule_revision {
        let page = log
            .graph_updates(id, schedule.revision, control.schedule_revision)
            .await
            .map_err(failure)?;
        if page.is_empty() {
            return Err(Error::Provider(
                "Graph schedule prefix is incomplete".into(),
            ));
        }
        for committed in page {
            schedule.apply(&committed).map_err(failure)?;
            // This private read projection keeps previews, never executable
            // instructions. Preserve explicit truncation evidence.
            for item in committed.update.add_work {
                if let Some(stored) = schedule.work.get_mut(&item.work_id) {
                    let text = &mut stored.work.instruction;
                    if text.len() > 1024 {
                        truncated.insert(item.work_id);
                        text.truncate(text.floor_char_boundary(1024));
                    }
                }
            }
        }
    }
    let namespace = Namespace::new(super::ID, Scope::Session(root.into())).map_err(failure)?;
    let mut work = Vec::new();
    let mut bytes = 0;
    let mut more = false;
    if after.is_some_and(|cursor| !schedule.work.contains_key(&cursor.work_id)) {
        return Err(Error::Invalid("Graph work cursor does not exist".into()));
    }
    for (work_id, item) in &schedule.work {
        if after.is_some_and(|cursor| work_id <= &cursor.work_id) {
            continue;
        }
        if work.len() == 16 {
            more = true;
            break;
        }
        let execution = if let Some(intent) =
            log.graph_intent(id, work_id).await.map_err(failure)?
        {
            if let Some(receipt) = log
                .plugin_execution_receipt(&namespace, &intent.request.operation_id)
                .await
                .map_err(failure)?
            {
                let observation = log
                    .plugin_execution_progress(receipt)
                    .await
                    .map_err(failure)?;
                use maka_graph::view::WorkState;
                let state = match observation.progress {
                    Progress::Pending => WorkState::Requested,
                    Progress::Running => WorkState::Running,
                    Progress::WaitingForUser | Progress::Paused => WorkState::Blocked,
                    Progress::Ended { outcome } => match outcome {
                        InvocationOutcome::Completed
                        | InvocationOutcome::ContextCompactFinished { .. } => WorkState::Completed,
                        InvocationOutcome::Failed { .. } => WorkState::Failed,
                        InvocationOutcome::Cancelled { .. } => WorkState::Cancelled,
                        InvocationOutcome::HandoffPaused { .. } => WorkState::Blocked,
                    },
                };
                Some(Execution {
                    result_record_id: observation.terminal_event_id,
                    session_id: observation.receipt.invocation.session_id,
                    turn_id: observation.receipt.invocation.turn_id,
                    state,
                })
            } else {
                None
            }
        } else {
            None
        };
        let row = Work {
            work_id: work_id.clone(),
            instruction: item.work.instruction.clone(),
            instruction_truncated: truncated.contains(work_id),
            status: item.status,
            execution,
        };
        let size = serde_json::to_vec(&row).map_err(failure)?.len();
        if !work.is_empty() && bytes + size > 48 * 1024 {
            more = true;
            break;
        }
        bytes += size;
        work.push(row);
    }
    let next_after = more.then(|| Cursor {
        revision: schedule.revision,
        work_id: work.last().expect("nonempty Graph page").work_id.clone(),
    });
    Ok(Some(Graph {
        epoch: control.epoch,
        revision: schedule.revision,
        stop_requested: control.stop_requested,
        finished: control.finished,
        selected_result_ids: schedule
            .finish
            .map_or_else(Vec::new, |(finish, _, _)| finish.result_ids),
        total_work: schedule.work.len(),
        work,
        next_after,
    }))
}
fn failure(error: impl ToString) -> Error {
    Error::Provider(error.to_string())
}
