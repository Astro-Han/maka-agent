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

use crate::{Content, Message, ProjectionError, Row, watermark};
use maka_runtime::workhub::ActionId;
use maka_runtime::{
    session_event::{SessionEvent, SessionFact},
    workhub::{StopIntent, StopOutcome},
};
use serde::Serialize;

mod correction;
pub use correction::correction;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CoordinationRecord {
    Stop(Box<StopMessage>),
    Assigned(Box<AssignmentMessage>),
    Correction(Box<correction::CorrectionMessage>),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentMessage {
    schema_version: u8,
    kind: AssignmentKind,
    action_id: ActionId,
    action_fingerprint: String,
    coordination_turn_id: String,
    delegation_id: String,
    target_session_id: String,
    target_turn_id: String,
    target_message_id: String,
    target_session_name: String,
    disposition: Disposition,
    user_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<maka_runtime::attachment::AttachmentRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_attachments: Option<Vec<maka_runtime::attachment::AttachmentRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delegation_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    create: Option<maka_runtime::workhub::CreateSpec>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    steered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    replaces_action_id: Option<ActionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replaces_delegation_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AssignmentKind {
    DelegationAssigned,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Disposition {
    DelegateExisting,
    CreateNew,
}

/// Project one assignment using its caller-authenticated original user message.
/// A physical handoff opening does not itself contain that message.
pub fn delegated(
    stored: &maka_runtime::event::StoredEvent,
    source: &maka_runtime::input::MessageInput,
) -> Result<Option<Row>, ProjectionError> {
    let maka_runtime::event::Fact::WorkhubDelegated { delegation } = &stored.event.fact else {
        return Err(ProjectionError::Invalid("not a WorkHub delegation"));
    };
    let sequence = watermark(stored.sequence)? - 255;
    Ok(assigned(&stored.event, delegation, source)?.map(|message| Row { sequence, message }))
}

pub(crate) fn assigned(
    event: &maka_runtime::event::RuntimeEvent,
    delegation: &maka_runtime::workhub::Delegation,
    source: &maka_runtime::input::MessageInput,
) -> Result<Option<Message>, ProjectionError> {
    assignment(
        &event.id,
        event.recorded_at,
        &event.invocation,
        delegation,
        source,
        None,
    )
}

fn assignment(
    id: &str,
    recorded_at: std::time::SystemTime,
    coordinator: &maka_runtime::event::Invocation,
    delegation: &maka_runtime::workhub::Delegation,
    source: &maka_runtime::input::MessageInput,
    replaces: Option<(&ActionId, &str)>,
) -> Result<Option<Message>, ProjectionError> {
    use maka_runtime::workhub::DelegationDescription;
    delegation
        .validate(coordinator)
        .map_err(ProjectionError::Invalid)?;
    // Older facts have no canonical display description. Never substitute
    // today's mutable Session metadata for missing historical evidence.
    let Some(description) = &delegation.description else {
        return Ok(None);
    };
    let (disposition, create) = match description {
        DelegationDescription::Existing { .. } => (Disposition::DelegateExisting, None),
        DelegationDescription::Created { spec, .. } => (Disposition::CreateNew, Some(spec.clone())),
    };
    let target_attachments = source
        .attachments
        .as_ref()
        .map(|items| {
            items
                .iter()
                .map(|item| delegation.attachment(item))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map_err(ProjectionError::Invalid)?;
    Ok(Some(Message {
        id: id.into(),
        turn_id: coordinator.turn_id.clone(),
        ts: crate::message::capture_time(recorded_at)?,
        content: Content::WorkhubCoordination {
            record: CoordinationRecord::Assigned(Box::new(AssignmentMessage {
                schema_version: if replaces.is_some() { 2 } else { 1 },
                kind: AssignmentKind::DelegationAssigned,
                action_id: delegation.action_id.clone(),
                action_fingerprint: delegation.request_fingerprint.clone(),
                coordination_turn_id: coordinator.turn_id.clone(),
                delegation_id: id.into(),
                target_session_id: delegation.target.session_id.clone(),
                target_turn_id: delegation.target.turn_id.clone(),
                target_message_id: delegation.target_message_id(),
                target_session_name: description.name().into(),
                disposition,
                user_text: source.text.clone(),
                attachments: source.attachments.clone(),
                target_attachments,
                delegation_text: (delegation.delegation_text != source.text)
                    .then(|| delegation.delegation_text.clone()),
                create,
                steered: delegation.delivery.is_steering(),
                replaces_action_id: replaces.map(|(action, _)| action.clone()),
                replaces_delegation_id: replaces.map(|(_, delegation)| delegation.into()),
            })),
        },
    }))
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopMessage {
    schema_version: u8,
    action_id: ActionId,
    action_fingerprint: String,
    coordination_turn_id: String,
    stops_action_id: ActionId,
    stops_delegation_id: String,
    target_session_id: String,
    #[serde(flatten)]
    detail: StopDetail,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum StopDetail {
    DelegationStopRequested {
        target_message_id: String,
        target_session_name: String,
        user_text: String,
    },
    DelegationStopResolved {
        outcome: StopOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_turn_id: Option<String>,
    },
}

/// Control results are new immutable rows, even after the correlated Run ended.
pub fn stop(
    sequence: u64,
    event: &SessionEvent,
    intent: &StopIntent,
    delegation_id: &str,
    target_message_id: &str,
) -> Result<Row, ProjectionError> {
    event.validate().map_err(ProjectionError::Invalid)?;
    intent.validate().map_err(ProjectionError::Invalid)?;
    if event.turn_id != intent.request.source.turn_id {
        return Err(ProjectionError::Invalid("WorkHub stop correlation changed"));
    }
    let detail = match &event.fact {
        SessionFact::WorkhubStopRequested {
            intent: recorded,
            target_session_name,
            user_text,
        } => {
            if recorded.as_ref() != intent {
                return Err(ProjectionError::Invalid("WorkHub stop intent changed"));
            }
            StopDetail::DelegationStopRequested {
                target_message_id: target_message_id.into(),
                target_session_name: target_session_name.clone(),
                user_text: user_text.clone(),
            }
        }
        SessionFact::WorkhubStopResolved {
            action_id,
            resolution,
        } => {
            if *action_id != intent.request.action_id {
                return Err(ProjectionError::Invalid("WorkHub stop action changed"));
            }
            StopDetail::DelegationStopResolved {
                outcome: resolution.outcome(),
                target_turn_id: resolution.target_turn_id().map(str::to_owned),
            }
        }
        _ => return Err(ProjectionError::Invalid("not a WorkHub stop fact")),
    };
    Ok(Row {
        sequence: watermark(sequence)? - 255,
        message: Message {
            id: event.id.clone(),
            turn_id: event.turn_id.clone(),
            ts: crate::message::capture_time(event.recorded_at)?,
            content: Content::WorkhubCoordination {
                record: CoordinationRecord::Stop(Box::new(StopMessage {
                    schema_version: 3,
                    action_id: intent.request.action_id.clone(),
                    action_fingerprint: intent.request.request_fingerprint.clone(),
                    coordination_turn_id: event.turn_id.clone(),
                    stops_action_id: intent.delegation_action_id.clone(),
                    stops_delegation_id: delegation_id.into(),
                    target_session_id: intent.request.target_session_id.clone(),
                    detail,
                })),
            },
        },
    })
}
