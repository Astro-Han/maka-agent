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

use super::{
    Code, Host, Input, Operation, OperationError, Output, failure, fingerprint, storage_error,
};
use maka_event_log::{
    message_queue::{QueueCommand, QueueCommandKind, QueueEdit, QueueReceipt},
    turns::InvocationState,
};
use maka_protocol::message::{EntryState, MutationResult, RetractResult};

pub(super) async fn execute(
    host: &Host,
    connection_id: uuid::Uuid,
    operation: Operation,
    input: Input,
) -> Result<Output, OperationError> {
    let mut prepared: Option<(
        Option<maka_runtime::event::Invocation>,
        Result<crate::execution::input::PreparedMessageInput, OperationError>,
    )> = None;
    loop {
        let admission = host.executions.lock_admission().await;
        if host.draining.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let (epoch, session, id) = match &input {
            Input::Retract(i) => (&i.origin_host_epoch, &i.session_id, &i.retract_id),
            Input::RetractEntry(i) => (&i.origin_host_epoch, &i.session_id, &i.retract_id),
            Input::Promote(i) => (&i.origin_host_epoch, &i.session_id, &i.promote_id),
            Input::Update(i) => (&i.origin_host_epoch, &i.session_id, &i.update_id),
            Input::Reorder(i) => (&i.origin_host_epoch, &i.session_id, &i.reorder_id),
            _ => unreachable!("installed queue mutation"),
        };
        if epoch != &host.epoch {
            return Err(failure(
                Code::OutcomeUnknown,
                "Queue command outcome is not durable across Host Epochs",
            ));
        }
        let command = QueueCommand {
            host_epoch: epoch.clone(),
            command_id: id.clone(),
            kind: match &input {
                Input::Retract(_) => QueueCommandKind::RetractAll,
                Input::RetractEntry(_) => QueueCommandKind::Retract,
                Input::Promote(_) => QueueCommandKind::Promote,
                Input::Update(_) => QueueCommandKind::Update,
                Input::Reorder(_) => QueueCommandKind::Reorder,
                _ => unreachable!("installed queue mutation"),
            },
            fingerprint: fingerprint(&input)?,
        };
        if let Some(receipt) = host
            .log
            .queue_command_receipt(session, &command)
            .await
            .map_err(|e| storage_error(host, e))?
        {
            return Ok(output(&host.epoch, operation, receipt));
        }
        let observation = host
            .log
            .session_projection::<crate::session::SessionConfiguration>(session)
            .await
            .map_err(|e| storage_error(host, e))?
            .ok_or_else(|| failure(Code::NotFound, "Session does not exist"))?;
        if observation.session.archived {
            return Err(failure(Code::SessionArchived, "Session is archived"));
        }
        let queue = &observation.message_queue;
        let candidate = super::projection::project(&host.epoch, queue);
        let mut _input_admission = None;
        let edit = match &input {
            Input::Retract(_) => QueueEdit::RetractAll {
                cancellation_id: id.clone(),
            },
            Input::RetractEntry(i) => {
                require_entry(&candidate, &i.entry_id)?;
                QueueEdit::Retract {
                    message_id: i.entry_id.clone(),
                    cancellation_id: id.clone(),
                }
            }
            Input::Promote(i) => {
                require_entry(&candidate, &i.entry_id)?;
                if !host.executions.has_active_session(session) {
                    return Err(failure(
                        Code::OperationConflict,
                        "No live Run can accept steering",
                    ));
                }
                let root = observation
                    .root_turn
                    .as_ref()
                    .filter(|root| !matches!(root.state, InvocationState::Ended { .. }))
                    .ok_or_else(|| {
                        failure(
                            Code::OperationConflict,
                            "No active Turn can accept steering",
                        )
                    })?;
                let tools = host
                    .executions
                    .active_tool_names(&root.invocation)
                    .ok_or_else(|| {
                        failure(
                            Code::OperationConflict,
                            "Steering target is no longer active",
                        )
                    })?;
                let entry = queue
                    .entries
                    .iter()
                    .find(|entry| entry.source.message.message_id == i.entry_id)
                    .expect("checked entry");
                if !entry.required_tools.iter().all(|name| tools.contains(name)) {
                    return Err(failure(
                        Code::OperationConflict,
                        "Active Run lacks tools required by the queued input",
                    ));
                }
                QueueEdit::Promote {
                    message_id: i.entry_id.clone(),
                    invocation: root.invocation.clone(),
                }
            }
            Input::Update(i) => {
                require_entry(&candidate, &i.entry_id)?;
                if queue.revision != i.expected_queue_revision {
                    return Err(failure(
                        Code::OperationConflict,
                        "Message queue changed since editing began",
                    ));
                }
                let entry = queue
                    .entries
                    .iter()
                    .find(|e| e.source.message.message_id == i.entry_id)
                    .expect("checked entry");
                let mut source = entry.source.clone();
                let required_tools;
                source.message.content = super::update::content(source.message.content, &i.text)?;
                if let Some(references) = &mut source.message.content.inline_references {
                    references.retain(|reference| {
                        reference.kind != maka_runtime::input::InlineReferenceKind::Skill
                    });
                }
                {
                    let active_tools = (source.disposition
                        == maka_runtime::message::MessageDisposition::Steering)
                        .then(|| host.executions.active_tool_names(entry.steering_target()))
                        .flatten();
                    let owner = active_tools
                        .as_ref()
                        .map(|_| entry.steering_target().clone());
                    let candidate = match prepared.take() {
                        Some((expected_owner, candidate)) if expected_owner == owner => {
                            let Some((candidate, guard)) =
                                candidate?.commit(&host.executions, session).await?
                            else {
                                continue;
                            };
                            _input_admission = guard;
                            candidate
                        }
                        _ => {
                            drop(admission);
                            let candidate = host
                                .executions
                                .prepare_message_input(
                                    observation.session,
                                    source.message.content.clone(),
                                    Some(connection_id),
                                    active_tools,
                                )
                                .await;
                            prepared = Some((owner, candidate));
                            continue;
                        }
                    };
                    source.message.content = candidate.content;
                    host.executions
                        .validate_message_content(
                            session,
                            &source.message.content.clone().into(),
                            host.root_id(),
                        )
                        .await?;
                    match candidate.selection {
                        crate::execution::input::Outcome::Ready {
                            required_tools: requirements,
                        } => {
                            required_tools = requirements;
                        }
                        crate::execution::input::Outcome::Blocked { message } => {
                            return Err(failure(Code::OperationConflict, &message));
                        }
                    };
                }
                QueueEdit::Update {
                    required_tools,
                    message_id: i.entry_id.clone(),
                    content: Box::new(source.message.content),
                }
            }
            Input::Reorder(i) => QueueEdit::Reorder {
                message_ids: i.entry_ids.clone(),
            },
            _ => unreachable!("installed queue mutation"),
        };
        let revision = queue.revision;
        super::capacity::check(&host.epoch, observation, &edit)?;
        let change = host
            .log
            .edit_message_queue(session, revision, edit, command)
            .await
            .map_err(|e| storage_error(host, e))?;
        return Ok(output(&host.epoch, operation, change));
    }
}

fn output(epoch: &str, operation: Operation, receipt: QueueReceipt) -> Output {
    if operation == Operation::QueueRetract {
        let entries = super::projection::project(
            epoch,
            &maka_event_log::message_queue::MessageQueue {
                revision: receipt.revision,
                entries: receipt.retracted,
            },
        );
        let mut retracted = entries
            .steering
            .into_iter()
            .chain(entries.followup)
            .collect::<Vec<_>>();
        for entry in &mut retracted {
            entry.state = EntryState::Retracted;
        }
        Output::Retract(RetractResult {
            queue_revision: receipt.revision,
            retracted,
        })
    } else {
        Output::Mutation(MutationResult {
            queue_revision: receipt.revision,
        })
    }
}
fn require_entry(
    queue: &maka_protocol::message::QueueProjection,
    id: &str,
) -> Result<(), OperationError> {
    if queue
        .steering
        .iter()
        .chain(&queue.followup)
        .any(|entry| entry.entry_id == id)
    {
        Ok(())
    } else {
        Err(failure(
            Code::NotFound,
            "Message queue entry does not exist",
        ))
    }
}
