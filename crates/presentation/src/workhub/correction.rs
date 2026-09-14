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

use super::{Content, CoordinationRecord, Disposition, Message, ProjectionError, Row, watermark};
use maka_runtime::workhub::ActionId;
use maka_runtime::{
    input::MessageInput,
    session_event::{SessionEvent, SessionFact},
    workhub::{CorrectionAbort, CorrectionIntent, Delegation},
};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionMessage {
    schema_version: u8,
    action_id: ActionId,
    action_fingerprint: String,
    coordination_turn_id: String,
    #[serde(flatten)]
    detail: CorrectionDetail,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum CorrectionDetail {
    #[serde(rename = "delegation_replacement_requested")]
    Requested(Box<Requested>),
    #[serde(rename = "delegation_superseded")]
    Superseded {
        superseded_action_id: ActionId,
        superseded_delegation_id: String,
        replacement_delegation_id: String,
    },
    #[serde(rename = "delegation_replacement_aborted")]
    Aborted {
        aborted_action_id: ActionId,
        aborted_delegation_id: String,
        target_session_id: String,
        reason: CorrectionAbort,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Requested {
    replaces_action_id: ActionId,
    replaces_delegation_id: String,
    replaced_target_session_id: String,
    replaced_target_message_id: String,
    target_session_id: String,
    target_session_name: String,
    disposition: Disposition,
    user_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<maka_runtime::attachment::AttachmentRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delegation_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    create: Option<maka_runtime::workhub::CreateSpec>,
}

pub fn correction(
    sequence: u64,
    event: &SessionEvent,
    intent: &CorrectionIntent,
    replaced_id: &str,
    replaced: &Delegation,
    source: &MessageInput,
) -> Result<Row, ProjectionError> {
    event.validate().map_err(ProjectionError::Invalid)?;
    intent.validate().map_err(ProjectionError::Invalid)?;
    let request = &intent.request;
    if event.turn_id != request.source.turn_id || replaced.action_id != request.replaces_action_id {
        return Err(ProjectionError::Invalid(
            "WorkHub correction correlation changed",
        ));
    }
    let detail = match &event.fact {
        SessionFact::WorkhubDelegated {
            coordinator,
            delegation,
            replaces_action_id,
        } => {
            if coordinator != &request.source
                || replaces_action_id != &request.replaces_action_id
                || delegation.action_id != request.action_id
            {
                return Err(ProjectionError::Invalid(
                    "WorkHub correction assignment changed",
                ));
            }
            let message = super::assignment(
                &event.id,
                event.recorded_at,
                coordinator,
                delegation,
                source,
                Some((replaces_action_id, replaced_id)),
            )?
            .ok_or(ProjectionError::Invalid(
                "WorkHub replacement description missing",
            ))?;
            return Ok(Row {
                sequence: watermark(sequence)? - 255,
                message,
            });
        }
        SessionFact::WorkhubCorrectionRequested { intent: recorded } => {
            if recorded.as_ref() != intent {
                return Err(ProjectionError::Invalid(
                    "WorkHub correction intent changed",
                ));
            }
            CorrectionDetail::Requested(Box::new(Requested {
                replaces_action_id: request.replaces_action_id.clone(),
                replaces_delegation_id: replaced_id.into(),
                replaced_target_session_id: replaced.target.session_id.clone(),
                replaced_target_message_id: replaced.target_message_id(),
                target_session_id: request.target.session_id().to_owned(),
                target_session_name: request.target.name().into(),
                disposition: match request.target.kind() {
                    maka_runtime::workhub::DelegationKind::Existing => {
                        Disposition::DelegateExisting
                    }
                    maka_runtime::workhub::DelegationKind::Created => Disposition::CreateNew,
                },
                user_text: source.text.clone(),
                attachments: source.attachments.clone(),
                delegation_text: (request.delegation_text != source.text)
                    .then(|| request.delegation_text.clone()),
                create: request.target.create().cloned(),
            }))
        }
        SessionFact::WorkhubSuperseded {
            action_id,
            replaces_action_id,
            replacement_delegation_id,
        } => {
            if action_id != &request.action_id || replaces_action_id != &request.replaces_action_id
            {
                return Err(ProjectionError::Invalid("WorkHub supersession changed"));
            }
            CorrectionDetail::Superseded {
                superseded_action_id: replaces_action_id.clone(),
                superseded_delegation_id: replaced_id.into(),
                replacement_delegation_id: replacement_delegation_id.clone(),
            }
        }
        SessionFact::WorkhubCorrectionAborted { action_id, reason } => {
            if action_id != &request.action_id {
                return Err(ProjectionError::Invalid("WorkHub correction abort changed"));
            }
            CorrectionDetail::Aborted {
                aborted_action_id: request.replaces_action_id.clone(),
                aborted_delegation_id: replaced_id.into(),
                target_session_id: request.target.session_id().to_owned(),
                reason: *reason,
            }
        }
        _ => return Err(ProjectionError::Invalid("not a WorkHub correction fact")),
    };
    Ok(Row {
        sequence: watermark(sequence)? - 255,
        message: Message {
            id: event.id.clone(),
            turn_id: event.turn_id.clone(),
            ts: crate::message::capture_time(event.recorded_at)?,
            content: Content::WorkhubCoordination {
                record: CoordinationRecord::Correction(Box::new(CorrectionMessage {
                    schema_version: 2,
                    action_id: request.action_id.clone(),
                    action_fingerprint: request.request_fingerprint.clone(),
                    coordination_turn_id: request.source.turn_id.clone(),
                    detail,
                })),
            },
        },
    })
}
