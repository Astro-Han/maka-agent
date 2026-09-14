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

use super::{Code, OperationError, failure};
use maka_event_log::message_queue::QueueEdit;
use maka_runtime::message::{self, MessageDisposition as Disposition};
pub(super) fn check(
    epoch: &str,
    mut observation: maka_event_log::observation::SessionProjection<
        crate::session::SessionConfiguration,
    >,
    edit: &QueueEdit,
) -> Result<(), OperationError> {
    let prospective = &mut observation.message_queue;
    if matches!(edit, QueueEdit::RetractAll { .. }) {
        retraction(super::projection::project(epoch, prospective))?;
    }
    match edit {
        QueueEdit::Promote { message_id, .. } => {
            prospective
                .entries
                .iter_mut()
                .find(|e| &e.source.message.message_id == message_id)
                .expect("validated entry")
                .source
                .disposition = Disposition::Steering;
        }
        QueueEdit::Update {
            message_id,
            content,
            submitted_content_digest,
            skill_invocation,
            required_tools,
        } => {
            let entry = prospective
                .entries
                .iter_mut()
                .find(|e| &e.source.message.message_id == message_id)
                .expect("validated entry");
            entry.source.message.content = (**content).clone();
            entry.source.message.submitted_content_digest = submitted_content_digest.clone();
            entry.source.skill_invocation = skill_invocation.clone();
            entry.required_tools = required_tools.clone();
        }
        QueueEdit::RetractAll { .. } => prospective
            .entries
            .retain(|entry| entry.source.disposition == Disposition::TurnStarted),
        QueueEdit::Retract { message_id, .. } => prospective
            .entries
            .retain(|entry| &entry.source.message.message_id != message_id),
        QueueEdit::Reorder { .. } => {}
    }
    validate(epoch, observation)
}

pub(crate) fn validate(
    epoch: &str,
    mut observation: maka_event_log::observation::SessionProjection<
        crate::session::SessionConfiguration,
    >,
) -> Result<(), OperationError> {
    let prospective = &mut observation.message_queue;
    let mut projection = super::projection::project(epoch, prospective);
    if let Some(root) = &observation.root_turn {
        interrupt(projection.clone(), &root.invocation)?;
    }
    projection.queue_revision = maka_runtime::configuration::validation::MAX_SAFE_INTEGER;
    for entry in &mut projection.steering {
        entry.state = maka_protocol::message::EntryState::InFlight;
    }
    let value = serde_json::to_value(&projection)
        .map_err(|e| failure(Code::InternalFailure, &e.to_string()))?;
    maka_protocol::message::decode_queue_projection(&value).map_err(|_| {
        failure(
            Code::SessionBusy,
            "Message queue projection capacity is full",
        )
    })?;
    retraction(projection)?;
    let steering: Vec<_> = prospective
        .entries
        .iter()
        .filter(|e| e.source.disposition == Disposition::Steering)
        .map(|e| e.source.clone())
        .collect();
    if !steering.is_empty() {
        message::validate_sources(
            &message::aggregate(steering.iter().map(|s| &s.message.content)),
            &steering,
        )
        .map_err(|_| {
            failure(
                Code::SessionBusy,
                "Steering successor exceeds durable capacity",
            )
        })?;
    }
    for entry in prospective
        .entries
        .iter()
        .filter(|e| e.source.disposition == Disposition::Followup)
    {
        let source = &entry.source;
        message::validate_sources(
            &message::aggregate([&source.message.content]),
            std::slice::from_ref(source),
        )
        .map_err(|_| failure(Code::SessionBusy, "Followup exceeds durable capacity"))?;
    }
    prospective.revision = maka_runtime::configuration::validation::MAX_SAFE_INTEGER;
    super::super::subscriptions::delivery::project(
        epoch,
        maka_runtime::configuration::validation::MAX_SAFE_INTEGER,
        observation,
    )
    .and_then(|snapshot| snapshot.validate().map_err(Into::into))
    .map_err(|_| failure(Code::SessionBusy, "Session projection capacity is full"))
}

fn retraction(projection: maka_protocol::message::QueueProjection) -> Result<(), OperationError> {
    let result = maka_protocol::message::RetractResult {
        queue_revision: maka_runtime::configuration::validation::MAX_SAFE_INTEGER,
        retracted: super::projection::retracted(projection),
    };
    if serde_json::to_vec(&result)
        .map_err(|e| failure(Code::InternalFailure, &e.to_string()))?
        .len()
        > maka_protocol::message::MAX_RESULT_BYTES
    {
        return Err(failure(
            Code::SessionBusy,
            "Message result exceeds byte capacity",
        ));
    }
    Ok(())
}

pub(crate) fn interrupt(
    projection: maka_protocol::message::QueueProjection,
    identity: &maka_runtime::event::Invocation,
) -> Result<(), OperationError> {
    let retracted = super::projection::retracted(projection);
    // Protocol-valid escaped failure text has a larger wire representation than
    // its raw byte limit. Reserve it at admission, not when the user needs to stop.
    let value = serde_json::json!({
        "queueRevision": maka_runtime::configuration::validation::MAX_SAFE_INTEGER,
        "retracted": retracted,
        "turn": {
            "sessionId": identity.session_id, "turnId": identity.turn_id, "runId": identity.run_id,
            "status": "failed", "terminalEventId": "x".repeat(128),
            "failureClass": "\0".repeat(128), "failureMessage": "\0".repeat(256)
        }
    });
    if serde_json::to_vec(&value)
        .map_err(|e| failure(Code::InternalFailure, &e.to_string()))?
        .len()
        > maka_protocol::message::MAX_RESULT_BYTES
    {
        return Err(failure(
            Code::SessionBusy,
            "Interrupt result exceeds byte capacity",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{PreparedSession, SessionModel};
    use maka_event_log::{EventLog, message_admissions::PendingMessageAdmission};
    use maka_runtime::{
        event::{EventWrite, Fact, Invocation, InvocationInput, RuntimeEvent},
        input::DeliveredMessage,
        interaction::{InteractionQuestion, InteractionRecord, InteractionRequest, QuestionOption},
        message::{Placement, RootSourceMessage},
    };
    use serde_json::json;
    #[tokio::test]
    async fn queue_growth_must_leave_the_complete_interaction_snapshot_deliverable() {
        let temp = tempfile::tempdir().unwrap();
        let log = EventLog::open(&temp.path().join("capacity.sqlite"))
            .await
            .unwrap();
        let configuration = PreparedSession::new(
            serde_json::from_value(json!({
                "sessionId":"session", "workspace":{"kind":"host_path", "path":temp.path()},
                "modelTarget":{"kind":"default"}
            }))
            .unwrap(),
        )
        .unwrap()
        .bind(
            maka_protocol::session::WorkspaceProjection {
                target: maka_protocol::session::WorkspaceTarget::HostPath {
                    path: temp.path().to_string_lossy().into_owned(),
                },
                host_cwd: temp.path().to_string_lossy().into_owned(),
            },
            SessionModel {
                connection_id: "fixture".into(),
                connection_slug: "fixture".into(),
                model: "fixture".into(),
            },
            maka_protocol::session::PermissionMode::Explore,
            maka_runtime::execution::ToolMode::Direct,
        );
        log.create_session("session", "create", &configuration, 1)
            .await
            .unwrap();
        let invocation = Invocation {
            session_id: "session".into(),
            turn_id: "turn".into(),
            run_id: "run".into(),
            invocation_id: "invocation".into(),
        };
        log.append(
            &EventWrite::plain(RuntimeEvent::new(
                invocation.clone(),
                Fact::InvocationOpened {
                    configuration: None,
                    input: InvocationInput::Message {
                        content: "root".into(),
                        request_fingerprint: None,
                        skill_invocation: Default::default(),
                        source_messages: Vec::new(),
                    },
                },
            ))
            .unwrap(),
        )
        .await
        .unwrap();
        log.admit_message(PendingMessageAdmission {
            invocation,
            steering_invocation: None,
            required_tools: Default::default(),
            admitted_at: 2,
            source: RootSourceMessage {
                message: DeliveredMessage {
                    message_id: "queued".into(),
                    content: "short".into(),
                    submitted_content_digest: format!("sha256:{}", "a".repeat(64)),
                },
                submitted_placement: Placement::NextTurn,
                disposition: Disposition::Followup,
                skill_invocation: Default::default(),
                submitted_intent: None,
            },
        })
        .await
        .unwrap();
        let request = InteractionRecord {
            session_id: "session".into(),
            turn_id: "turn".into(),
            run_id: "run".into(),
            request_id: "request".into(),
            created_at: 3,
            outcome: None,
            request: InteractionRequest::Question {
                tool_use_id: "tool".into(),
                questions: (0..3)
                    .map(|_| InteractionQuestion {
                        question: "q".repeat(1024),
                        options: (0..3)
                            .map(|i| QuestionOption {
                                label: format!("{i}{}", "l".repeat(255)),
                                description: Some("d".repeat(512)),
                            })
                            .collect(),
                    })
                    .collect(),
            },
        };
        log.establish_interaction(&request).await.unwrap();
        let before = log.message_queue("session").await.unwrap();
        for (bytes, fits) in [(40 * 1024, true), (48 * 1024, false)] {
            let observation = log.session_projection("session").await.unwrap().unwrap();
            let edit = QueueEdit::Update {
                message_id: "queued".into(),
                content: Box::new("x".repeat(bytes).into()),
                submitted_content_digest: format!("sha256:{}", "b".repeat(64)),
                skill_invocation: Default::default(),
                required_tools: Default::default(),
            };
            let result = check("epoch", observation, &edit);
            assert_eq!(result.is_ok(), fits);
            if let Err(error) = result {
                assert_eq!(error.code, Code::SessionBusy);
                assert_eq!(error.message, "Session projection capacity is full");
            }
        }
        assert_eq!(log.message_queue("session").await.unwrap(), before);
        log.close().await.unwrap();
    }
}
