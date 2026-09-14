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

use super::{InteractionRequest, decode_request};
use crate::{ProtocolError, Result, display::project};
use serde_json::{Value, json};

/// Decode before display projection, then validate the projected request again.
/// Canonical reads must use decode_request, preserving their stored display text.
pub fn project_question_request(
    tool_use_id: &str,
    questions: &Value,
) -> Result<InteractionRequest> {
    let mut request = decode_request(&json!({
        "kind": "question", "toolUseId": tool_use_id, "questions": questions
    }))?;
    if let InteractionRequest::Question { questions, .. } = &mut request {
        for question in questions {
            question.question = project(&question.question, 1024)?;
            for option in &mut question.options {
                option.label = project(&option.label, 256)?;
                if let Some(description) = &mut option.description {
                    *description = project(description, 512)?;
                }
            }
        }
    }
    request.validate().map_err(ProtocolError::invalid)?;
    Ok(request)
}
