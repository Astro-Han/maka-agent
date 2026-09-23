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

use crate::pages::sending::Submission;
use maka_protocol::{message::Placement, turn::MessageContent};
use serde::{Deserialize, Serialize};

/// v1-v6 saved text-only requests. Never reinterpret an incomplete v7 request
/// as legacy: losing metadata would change an unknown request's identity.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum Saved {
    Current(Box<Submission>),
    Legacy(Legacy),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Legacy {
    root_id: String,
    origin_epoch: String,
    session: String,
    id: String,
    text: String,
    placement: Placement,
}

impl Saved {
    pub fn request(self, version: u32) -> Result<Submission, String> {
        match self {
            Self::Current(request) if (7..=8).contains(&version) => Ok(*request),
            Self::Legacy(request) if (1..=6).contains(&version) => Ok(Submission {
                root_id: request.root_id,
                origin_epoch: request.origin_epoch,
                session: request.session,
                id: request.id,
                content: MessageContent {
                    text: request.text,
                    display_text: None,
                    attachments: None,
                    directory_references: None,
                    quotes: None,
                    inline_references: None,
                },
                placement: request.placement,
                input_selections: Default::default(),
                turn_orchestration: None,
            }),
            _ => Err("Submission does not match checkpoint version".into()),
        }
    }
}
