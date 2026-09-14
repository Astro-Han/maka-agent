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

use crate::display::project;
use crate::{ProtocolError, Result};
use maka_runtime::capability::*;
use serde_json::Value;
fn invalid() -> ProtocolError {
    ProtocolError::invalid("Invalid Client Capability form")
}
fn decode_input(value: &Value) -> Result<FormInput> {
    let input = maka_runtime::capability::form_decode::decode_input(value)
        .map_err(ProtocolError::invalid)?;
    super::encoded_limit(
        &serde_json::json!({"kind":"form","toolUseId":"client-capability-interaction",
        "message":input.message,"requester":input.requester,"fields":input.fields}),
        16 * 1024,
    )?;
    Ok(input)
}
/// Validate both before and after display projection. Identity values are preserved.
/// Secret redaction is deliberately excluded from the Rust port.
pub fn decode_form_input(value: &Value) -> Result<FormInput> {
    let mut input = decode_input(value)?;
    input.message = project(&input.message, 2048)?;
    input.requester.name = project(&input.requester.name, 256)?;
    if let Some(source) = &mut input.requester.source {
        *source = project(source, 512)?;
    }
    for field in &mut input.fields {
        field.label = project(&field.label, 256)?;
        if let Some(description) = &mut field.description {
            *description = project(description, 512)?;
        }
        match &mut field.spec {
            FormFieldSpec::String { default, .. } => {
                if let Some(value) = default.as_ref()
                    && project(value, 2048)? != *value
                {
                    *default = None;
                }
            }
            FormFieldSpec::SingleSelect { options, .. }
            | FormFieldSpec::MultiSelect { options, .. } => {
                for option in options {
                    option.label = project(&option.label, 256)?;
                }
            }
            _ => {}
        }
    }
    decode_input(&serde_json::to_value(input).map_err(|_| invalid())?)
}
pub fn decode_form_result(value: &Value) -> Result<FormResult> {
    maka_runtime::capability::form_decode::decode_form_result(value).map_err(ProtocolError::invalid)
}
