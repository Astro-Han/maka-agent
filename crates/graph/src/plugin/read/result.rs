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

mod text;

use crate::{GraphId, WorkId, store::Store as _};
use maka_plugins::{execution::ReadArtifact, remote::Error};
use maka_runtime::event::Fact;
use serde::{Deserialize, Serialize};
use text::{TextPage, Window};

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::plugin) enum Part {
    #[default]
    Answer,
    Patch,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::plugin) struct ResultPage {
    graph_id: GraphId,
    work_id: WorkId,
    record_id: String,
    part: Part,
    isolated_workspace: bool,
    #[serde(flatten)]
    text: TextPage,
}

pub(in crate::plugin) async fn page(
    source: super::Source<'_>,
    root: &str,
    graph: &GraphId,
    work: &WorkId,
    record: &str,
    offset: usize,
    part: Part,
) -> Result<Option<ResultPage>, Error> {
    let repository = source.repository;
    let commands = source.commands;
    crate::identity(record).map_err(super::failure)?;
    let control = match repository.control(root, graph).await {
        Ok(control) => control,
        Err(crate::Error::NotFound(_)) => return Ok(None),
        Err(error) => return Err(super::failure(error)),
    };
    let Some(intent) = repository
        .intent(graph, work)
        .await
        .map_err(super::failure)?
    else {
        return Ok(None);
    };
    let Some((observed, isolated_workspace)) = source.observe(root, &intent).await? else {
        return Ok(None);
    };
    let invocation = &observed.receipt.invocation;
    let Some(event) = commands
        .event(
            intent.request.operation_id.clone(),
            record.into(),
            observed.through_sequence,
        )
        .await
        .map_err(super::failure)?
    else {
        return Ok(None);
    };
    if control.epoch.mode == crate::Mode::Swarm
        && !matches!(event.event.fact, Fact::InvocationEnded { .. })
    {
        return Ok(None);
    }
    if matches!(part, Part::Patch) {
        if !matches!(&event.event.fact, Fact::InvocationEnded { outcome } if !matches!(outcome, maka_runtime::event::InvocationOutcome::HandoffPaused { .. }))
        {
            return Ok(None);
        }
        let id =
            maka_runtime::artifact::workspace_patch_id(&invocation.session_id, &invocation.turn_id);
        let Some(chunk) = commands
            .artifact(ReadArtifact {
                operation_id: intent.request.operation_id.clone(),
                artifact_id: id,
                offset: offset as u64,
                limit: 4096,
            })
            .await
            .map_err(super::failure)?
        else {
            return Ok(None);
        };
        if offset > chunk.total_bytes as usize {
            return Err(Error::Provider("patch offset exceeds its size".into()));
        }
        let valid = match std::str::from_utf8(&chunk.bytes) {
            Ok(text) => text.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => {
                return Err(Error::Provider(
                    "patch offset is not a UTF-8 boundary".into(),
                ));
            }
        };
        return Ok(Some(ResultPage {
            graph_id: graph.clone(),
            work_id: work.clone(),
            record_id: record.into(),
            part,
            isolated_workspace: true,
            text: TextPage {
                text: String::from_utf8(chunk.bytes[..valid].to_vec()).map_err(super::failure)?,
                offset,
                total_bytes: chunk.total_bytes as usize,
                next_offset: (offset + valid < chunk.total_bytes as usize)
                    .then_some(offset + valid),
            },
        }));
    }
    let window = match &event.event.fact {
        Fact::InvocationEnded { outcome } => {
            if matches!(
                outcome,
                maka_runtime::event::InvocationOutcome::HandoffPaused { .. }
            ) {
                return Ok(None);
            }
            let mut answer = Window::new(offset);
            let mut after = 0;
            loop {
                let page = commands
                    .events(intent.request.operation_id.clone(), after, event.sequence)
                    .await
                    .map_err(super::failure)?;
                for row in page.events {
                    if row.event.invocation.turn_id == invocation.turn_id
                        && matches!(
                            row.event.fact,
                            Fact::ModelCompleted { .. } | Fact::ExecutorCompleted { .. }
                        )
                        && let Some(next) = Window::fact(&row.event.fact, offset)
                    {
                        answer = next;
                    }
                }
                let Some(next) = page.next_after else {
                    break;
                };
                if next <= after {
                    return Err(Error::Provider(
                        "Graph result history did not advance".into(),
                    ));
                }
                after = next;
                tokio::task::yield_now().await;
            }
            answer.outcome(outcome);
            answer
        }
        fact => match Window::fact(fact, offset) {
            Some(window) => window,
            None => return Ok(None),
        },
    };
    Ok(Some(ResultPage {
        graph_id: graph.clone(),
        work_id: work.clone(),
        record_id: record.into(),
        part,
        isolated_workspace,
        text: window.finish()?,
    }))
}
