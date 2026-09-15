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

use crate::artifact::{Artifact, content_digest};
use crate::event::{CommitError, Fact, Invocation, RuntimeEvent, ToolOutcome};
use crate::tool_output::{RawToolResultRef, ToolOutput, ToolSuccess, encode_raw_tool_result};
use std::{sync::Arc, time::SystemTime};

/// One checked transaction submission: successful facts cannot omit their bytes.
#[derive(Clone, Debug)]
pub struct EventWrite {
    event: RuntimeEvent,
    raw_payload: Option<Arc<[u8]>>,
    projection_artifacts: Vec<ProjectionArtifactWrite>,
}

#[derive(Clone, Debug)]
pub struct ProjectionArtifactWrite {
    artifact: Artifact,
    bytes: Arc<[u8]>,
}

impl ProjectionArtifactWrite {
    pub(crate) fn new(artifact: Artifact, bytes: Vec<u8>) -> Self {
        Self {
            artifact,
            bytes: bytes.into(),
        }
    }
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl EventWrite {
    pub fn plain(event: RuntimeEvent) -> Result<Self, CommitError> {
        if let Fact::InvocationOpened { input, .. } = &event.fact {
            input
                .validate_inheritance(&event.invocation)
                .map_err(|reason| CommitError::Rejected(reason.into()))?;
        }
        if let Fact::InvocationOpened {
            input:
                crate::input::InvocationInput::Continuation {
                    request_fingerprint,
                    ..
                },
            ..
        } = &event.fact
            && !crate::archive::valid_projection_digest(request_fingerprint)
        {
            return Err(CommitError::Rejected(
                "invalid continuation request fingerprint".into(),
            ));
        }
        if let Fact::InvocationOpened {
            input:
                crate::input::InvocationInput::Message {
                    content,
                    source_messages,
                    skill_invocation,
                    ..
                },
            ..
        } = &event.fact
        {
            crate::message::validate_opening(content, source_messages, skill_invocation.as_deref())
                .map_err(|message| CommitError::Rejected(message.into()))?;
            if !source_messages.is_empty()
                && serde_json::to_vec(&event)
                    .map_err(|error| CommitError::Rejected(error.to_string()))?
                    .len()
                    > 1024 * 1024
            {
                return Err(CommitError::Rejected(
                    "root message opening exceeds durable capacity".into(),
                ));
            }
        }
        if let Fact::MessageSteered {
            message,
            skill_invocation,
        } = &event.fact
        {
            message
                .validate()
                .map_err(|message| CommitError::Rejected(message.into()))?;
            skill_invocation
                .validate()
                .map_err(|message| CommitError::Rejected(message.into()))?;
        }
        if let Fact::InvocationOpened {
            configuration: Some(configuration),
            ..
        } = &event.fact
            && let Some(prompt) = &configuration.system_prompt
        {
            prompt
                .validate()
                .map_err(|message| CommitError::Rejected(message.into()))?;
        }
        if let Fact::ToolResultArchived { placeholder } = &event.fact {
            placeholder
                .validate()
                .map_err(|message| CommitError::Rejected(message.into()))?;
        }
        if let Fact::ModelRequested {
            effective_source_digest: Some(digest),
            ..
        } = &event.fact
            && !crate::archive::valid_projection_digest(digest)
        {
            return Err(CommitError::Rejected(
                "invalid effective source digest".into(),
            ));
        }
        if let Fact::ModelRequested {
            context: Some(context),
            ..
        } = &event.fact
        {
            context
                .validate()
                .map_err(|message| CommitError::Rejected(message.into()))?;
        }
        if let Fact::ContextCheckpointRecorded { checkpoint } = &event.fact {
            checkpoint
                .validate()
                .map_err(|message| CommitError::Rejected(message.into()))?;
        }
        if matches!(
            event.fact,
            Fact::ToolSettled {
                outcome: ToolOutcome::Succeeded { .. },
                ..
            }
        ) {
            return Err(CommitError::Rejected(
                "successful tool fact requires its raw payload".into(),
            ));
        }
        Ok(Self {
            event,
            raw_payload: None,
            projection_artifacts: Vec::new(),
        })
    }

    /// Identity and capture time are supplied once and preserved across exact retries.
    pub fn tool_success(
        id: String,
        recorded_at: SystemTime,
        invocation: Invocation,
        operation_id: String,
        success: ToolSuccess,
    ) -> Result<(Self, ToolOutput), CommitError> {
        let success = success
            .normalize(&id, recorded_at, &invocation)
            .map_err(|message| CommitError::Rejected(message.into()))?;
        let output = success.output;
        let bytes = encode_raw_tool_result(&output)
            .map_err(|message| CommitError::Rejected(message.into()))?;
        let raw = RawToolResultRef {
            bytes: bytes.len() as u64,
            digest: content_digest(&bytes),
        };
        Ok((
            Self {
                event: RuntimeEvent {
                    id,
                    recorded_at,
                    invocation,
                    fact: Fact::ToolSettled {
                        operation_id,
                        outcome: ToolOutcome::Succeeded {
                            raw,
                            model_projection: success.projection,
                        },
                    },
                },
                raw_payload: Some(bytes.into()),
                projection_artifacts: success.artifacts,
            },
            output,
        ))
    }

    pub fn event(&self) -> &RuntimeEvent {
        &self.event
    }
    pub fn raw_payload(&self) -> Option<&[u8]> {
        self.raw_payload.as_deref()
    }
    pub fn projection_artifacts(&self) -> &[ProjectionArtifactWrite] {
        &self.projection_artifacts
    }
}
