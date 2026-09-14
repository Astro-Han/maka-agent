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

use super::{decode_form_result, encoded_limit, entity, validate_json};
use crate::{
    ProtocolError, Result,
    codec::{exact, record, shaped, string},
};
use maka_runtime::capability::HostFrame;
use serde_json::{Map, Value};

pub fn decode_host_frame(value: &Value) -> Result<HostFrame> {
    let fields = record(value, "Client Capability Host frame")?;
    match fields.get("kind").and_then(Value::as_str) {
        Some("client.capability.call") => {
            shaped(
                fields,
                &[
                    "kind",
                    "invocationId",
                    "registrationId",
                    "offerId",
                    "serverId",
                    "toolName",
                    "arguments",
                    "sessionId",
                    "turnId",
                    "toolCallId",
                ],
                &["cwd"],
            )?;
            let arguments = decode_input(&fields["arguments"], "arguments")?;
            Ok(HostFrame::Call {
                invocation_id: entity(&fields["invocationId"], "invocationId")?,
                registration_id: entity(&fields["registrationId"], "registrationId")?,
                offer_id: entity(&fields["offerId"], "offerId")?,
                server_id: string(&fields["serverId"], "serverId", 128)?,
                tool_name: string(&fields["toolName"], "toolName", 128)?,
                arguments,
                session_id: entity(&fields["sessionId"], "sessionId")?,
                turn_id: entity(&fields["turnId"], "turnId")?,
                tool_call_id: entity(&fields["toolCallId"], "toolCallId")?,
                cwd: fields
                    .get("cwd")
                    .map(|v| string(v, "cwd", 4096))
                    .transpose()?,
            })
        }
        Some("client.capability.service_call") => {
            exact(
                fields,
                &[
                    "kind",
                    "invocationId",
                    "registrationId",
                    "serviceId",
                    "version",
                    "method",
                    "input",
                ],
            )?;
            let input = decode_input(&fields["input"], "input")?;
            Ok(HostFrame::ServiceCall {
                invocation_id: entity(&fields["invocationId"], "invocationId")?,
                registration_id: entity(&fields["registrationId"], "registrationId")?,
                service_id: entity(&fields["serviceId"], "serviceId")?,
                version: string(&fields["version"], "version", 64)?,
                method: entity(&fields["method"], "method")?,
                input,
            })
        }
        Some(
            kind @ ("client.capability.cancel"
            | "client.capability.release"
            | "client.capability.admitted"),
        ) => {
            exact(fields, &["kind", "invocationId"])?;
            let invocation_id = entity(&fields["invocationId"], "invocationId")?;
            Ok(match kind {
                "client.capability.cancel" => HostFrame::Cancel { invocation_id },
                "client.capability.release" => HostFrame::Release { invocation_id },
                _ => HostFrame::Admitted { invocation_id },
            })
        }
        Some("client.capability.registration_release") => {
            exact(fields, &["kind", "registrationId"])?;
            Ok(HostFrame::RegistrationRelease {
                registration_id: entity(&fields["registrationId"], "registrationId")?,
            })
        }
        Some("client.capability.interaction_result") => {
            exact(fields, &["kind", "invocationId", "interactionId", "result"])?;
            Ok(HostFrame::InteractionResult {
                invocation_id: entity(&fields["invocationId"], "invocationId")?,
                interaction_id: entity(&fields["interactionId"], "interactionId")?,
                result: decode_form_result(&fields["result"])?,
            })
        }
        _ => Err(ProtocolError::invalid(
            "Invalid Client Capability Host frame kind",
        )),
    }
}

fn decode_input(value: &Value, label: &str) -> Result<Map<String, Value>> {
    validate_json(value)?;
    let input = record(value, label)?;
    encoded_limit(input, 40 * 1024)?;
    Ok(input.clone())
}
