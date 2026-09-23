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

use super::*;
use serde::{Deserialize, Serialize};

/// Typed dispatch for the observation variants implemented by the native Host.
/// Unsupported future variants remain explicit protocol errors.
#[derive(Debug, Clone, PartialEq)]
pub enum ObservationFrame {
    Projection(Box<SessionProjectionFrame>),
    Assistant(AssistantObservationFrame),
    Transcript(TranscriptAdvancedFrame),
    Tool(ToolObservationFrame),
    Resource(ResourceObservationFrame),
    Graph(AgentGraphChangedFrame),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum AgentGraphChangedFrame {
    #[serde(rename = "subscription.agent_graph_changed")]
    Changed {
        host_epoch: String,
        subscription_id: String,
        sequence: u64,
        root_session_id: String,
        graph_id: String,
        reason: AgentGraphChangedReason,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentGraphChangedReason {
    Observation,
    RuntimeActivity,
    Reconciled,
    Stopped,
}

/// PTY frames carry no main subscription sequence and no projection revision.
pub struct ObservationEnvelope<'a> {
    pub host_epoch: &'a str,
    pub subscription_id: &'a str,
    pub session_id: Option<&'a str>,
    pub sequence: Option<u64>,
}

impl ObservationFrame {
    pub fn envelope(&self) -> ObservationEnvelope<'_> {
        let (epoch, subscription, session, seq) = match self {
            Self::Projection(frame) => {
                let SessionProjectionFrame::SessionProjection {
                    host_epoch,
                    subscription_id,
                    sequence,
                    snapshot,
                } = frame.as_ref();
                (
                    host_epoch,
                    subscription_id,
                    Some(snapshot.session.session_id.as_str()),
                    Some(*sequence),
                )
            }
            Self::Assistant(AssistantObservationFrame::SessionDelta {
                host_epoch,
                subscription_id,
                sequence,
                session_id,
                ..
            })
            | Self::Tool(ToolObservationFrame::SessionEvent {
                host_epoch,
                subscription_id,
                sequence,
                session_id,
                ..
            })
            | Self::Transcript(TranscriptAdvancedFrame::TranscriptAdvanced {
                host_epoch,
                subscription_id,
                sequence,
                session_id,
                ..
            })
            | Self::Resource(ResourceObservationFrame::DomainChanged {
                host_epoch,
                subscription_id,
                sequence,
                session_id,
                ..
            }) => (
                host_epoch,
                subscription_id,
                Some(session_id.as_str()),
                Some(*sequence),
            ),
            Self::Assistant(AssistantObservationFrame::Closed {
                host_epoch,
                subscription_id,
                sequence,
                ..
            }) => (host_epoch, subscription_id, None, Some(*sequence)),
            Self::Resource(ResourceObservationFrame::PtyData {
                host_epoch,
                subscription_id,
                session_id,
                ..
            }) => (host_epoch, subscription_id, Some(session_id.as_str()), None),
            Self::Graph(AgentGraphChangedFrame::Changed {
                host_epoch,
                subscription_id,
                sequence,
                root_session_id,
                ..
            }) => (
                host_epoch,
                subscription_id,
                Some(root_session_id.as_str()),
                Some(*sequence),
            ),
        };
        ObservationEnvelope {
            host_epoch: epoch,
            subscription_id: subscription,
            session_id: session,
            sequence: seq,
        }
    }

    pub fn is_closed(&self) -> bool {
        matches!(
            self,
            Self::Assistant(AssistantObservationFrame::Closed { .. })
        )
    }
}

pub fn decode_observation_frame(value: &Value) -> Result<ObservationFrame> {
    match value["kind"].as_str() {
        Some("subscription.session_projection") => Ok(ObservationFrame::Projection(Box::new(
            decode_session_projection_frame(value)?,
        ))),
        Some("subscription.session_delta" | "subscription.closed") => Ok(
            ObservationFrame::Assistant(decode_assistant_observation_frame(value)?),
        ),
        Some("subscription.transcript_advanced") => Ok(ObservationFrame::Transcript(
            decode_transcript_advanced_frame(value)?,
        )),
        Some("subscription.session_event") => Ok(ObservationFrame::Tool(
            decode_tool_observation_frame(value)?,
        )),
        Some("subscription.session_domain_changed" | "subscription.runtime_resource_pty_data") => {
            Ok(ObservationFrame::Resource(
                decode_resource_observation_frame(value)?,
            ))
        }
        Some("subscription.agent_graph_changed") => {
            ensure(
                serde_json::to_vec(value)
                    .map_err(|e| ProtocolError::invalid(e.to_string()))?
                    .len()
                    <= SUBSCRIPTION_FRAME_MAX_BYTES,
                "Graph frame exceeds byte limit",
            )?;
            let frame = decode(value)?;
            let AgentGraphChangedFrame::Changed {
                host_epoch,
                subscription_id,
                sequence,
                root_session_id,
                graph_id,
                ..
            } = &frame;
            id(host_epoch)?;
            id(subscription_id)?;
            entity(root_session_id)?;
            entity(graph_id)?;
            ensure(*sequence > 0, "Invalid subscription sequence")?;
            Ok(ObservationFrame::Graph(frame))
        }
        _ => Err(ProtocolError::invalid("Unsupported observation frame")),
    }
}
