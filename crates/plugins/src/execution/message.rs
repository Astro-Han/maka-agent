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

use super::{CommandError, Progress};
use crate::{Error, name};
use maka_runtime::{event::Invocation, input::MessageInput, message::Placement};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Queue against an exact observed owner. A finished owner never turns this
/// request into a new root execution; use submit for independently accepted work.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Enqueue {
    pub operation_id: String,
    /// Canonical message identity, suitable for optimistic Client presentation.
    pub message_id: String,
    pub invocation: Invocation,
    pub content: MessageInput,
    pub placement: Placement,
}
impl Enqueue {
    pub fn validate(&self) -> Result<(), Error> {
        name(&self.operation_id)?;
        maka_runtime::interaction::entity_id(&self.message_id)
            .map_err(|reason| Error::Invalid(reason.into()))?;
        for id in [
            &self.invocation.session_id,
            &self.invocation.turn_id,
            &self.invocation.run_id,
            &self.invocation.invocation_id,
        ] {
            name(id)?;
        }
        if serde_json::to_vec(&self.content)
            .map_err(|e| Error::Invalid(e.to_string()))?
            .len()
            > 64 * 1024
        {
            return Err(Error::Invalid("queued input exceeds 64 KiB".into()));
        }
        maka_runtime::message::validate_sources(&self.content, &[])
            .map_err(|e| Error::Invalid(e.into()))
    }
    pub fn digest(&self) -> Result<String, Error> {
        self.validate()?;
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(|e| Error::Invalid(e.to_string()))?)
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageReceipt {
    pub invocation: Invocation,
    pub message_id: String,
    pub request_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum MessageState {
    Pending,
    Cancelled,
    Delivered {
        invocation: Invocation,
        /// False for steering or a batched Turn. Delivery is not exclusive ownership.
        exclusive: bool,
        progress: Box<Progress>,
        answer: Option<Excerpt>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Excerpt {
    pub text: String,
    pub complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageObservation {
    pub receipt: MessageReceipt,
    pub state: MessageState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMessage {
    pub session_id: String,
    pub message_id: String,
}
impl SessionMessage {
    pub fn validate(&self) -> Result<(), Error> {
        name(&self.session_id)?;
        name(&self.message_id)
    }
}

/// Retraction is about the exact message, never cancellation of its recipient.
/// Once delivered, observation reports the owner instead of stopping shared work.
pub type MessageResult = Result<MessageObservation, CommandError>;
