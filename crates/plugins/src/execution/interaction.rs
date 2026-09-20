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

use crate::Error;
use maka_runtime::{
    capability::{FormField, FormRequester},
    event::Invocation,
    interaction::{InteractionQuestion, InteractionRequest},
};
use serde::{Deserialize, Serialize};

/// User input only. Permission decisions remain exclusively Host-owned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Prompt {
    Question {
        questions: Vec<InteractionQuestion>,
    },
    Form {
        message: String,
        fields: Vec<FormField>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfferInteraction {
    pub operation_id: String,
    pub invocation: Invocation,
    pub prompt: Prompt,
}
impl OfferInteraction {
    /// Host supplies the authenticated package, never a plugin-selected requester.
    pub fn request(&self, package: &str) -> Result<InteractionRequest, Error> {
        crate::name(&self.operation_id)?;
        let request = match &self.prompt {
            Prompt::Question { questions } => InteractionRequest::Question {
                tool_use_id: self.operation_id.clone(),
                questions: questions.clone(),
            },
            Prompt::Form { message, fields } => InteractionRequest::Form {
                tool_use_id: self.operation_id.clone(),
                message: message.clone(),
                requester: FormRequester {
                    name: package.into(),
                    source: None,
                },
                fields: fields.clone(),
            },
        };
        request
            .validate()
            .map_err(|error| Error::Invalid(error.into()))?;
        Ok(request)
    }
}
