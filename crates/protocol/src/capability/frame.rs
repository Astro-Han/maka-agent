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

use super::{decode_result, encoded_limit, entity, form::decode_form_input};
use crate::{
    ProtocolError, Result,
    codec::{count, exact, record, string},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use maka_runtime::capability::{AdmissionEvidence, ClientFrame};
use serde_json::Value;

pub const MAX_RESULT_BYTES: u64 = 24 * 1024 * 1024;
pub const RESULT_CHUNK_MAX_BYTES: u64 = 36 * 1024;
pub const MAX_RESULT_CHUNKS: u64 = MAX_RESULT_BYTES.div_ceil(RESULT_CHUNK_MAX_BYTES);
pub const MAX_PROGRESS_TOTAL: u64 = 1024;
pub const INLINE_RESULT_MAX_BYTES: usize = 40 * 1024;

pub fn is_client_frame_kind(kind: &str) -> bool {
    matches!(
        kind,
        "client.capability.accepted"
            | "client.capability.rejected"
            | "client.capability.failed"
            | "client.capability.progress"
            | "client.capability.result"
            | "client.capability.result_start"
            | "client.capability.result_chunk"
            | "client.capability.interaction_request"
    )
}

pub fn is_host_frame_kind(kind: &str) -> bool {
    matches!(
        kind,
        "client.capability.call"
            | "client.capability.service_call"
            | "client.capability.cancel"
            | "client.capability.release"
            | "client.capability.registration_release"
            | "client.capability.admitted"
            | "client.capability.interaction_result"
    )
}

/// Decode one provider frame; transfer ordering and cumulative budgets belong to
/// the invocation owner, after this boundary validates the individual frame.
pub fn decode_client_frame(value: &Value) -> Result<ClientFrame> {
    let fields = record(value, "Client Capability client frame")?;
    let invocation_id = || {
        entity(
            fields.get("invocationId").unwrap_or(&Value::Null),
            "invocationId",
        )
    };
    match fields.get("kind").and_then(Value::as_str) {
        Some("client.capability.accepted") => {
            exact(fields, &["kind", "invocationId", "admissionEvidence"])?;
            Ok(ClientFrame::Accepted {
                invocation_id: invocation_id()?,
                admission_evidence: decode_admission_evidence(&fields["admissionEvidence"])?,
            })
        }
        Some(kind @ ("client.capability.rejected" | "client.capability.failed")) => {
            exact(fields, &["kind", "invocationId", "message"])?;
            let invocation_id = invocation_id()?;
            let message = string(&fields["message"], "message", 4096)?;
            Ok(if kind == "client.capability.rejected" {
                ClientFrame::Rejected {
                    invocation_id,
                    message,
                }
            } else {
                ClientFrame::Failed {
                    invocation_id,
                    message,
                }
            })
        }
        Some("client.capability.progress") => {
            exact(fields, &["kind", "invocationId", "current", "total"])?;
            let current = count(&fields["current"], "current")?;
            let total = count(&fields["total"], "total")?;
            if total == 0 || total > MAX_PROGRESS_TOTAL || current > total {
                return Err(ProtocolError::invalid(
                    "Invalid Client Capability progress bounds",
                ));
            }
            Ok(ClientFrame::Progress {
                invocation_id: invocation_id()?,
                current,
                total,
            })
        }
        Some("client.capability.result") => {
            exact(fields, &["kind", "invocationId", "result"])?;
            encoded_limit(&fields["result"], INLINE_RESULT_MAX_BYTES)?;
            Ok(ClientFrame::Result {
                invocation_id: invocation_id()?,
                result: decode_result(&fields["result"])?,
            })
        }
        Some("client.capability.result_start") => {
            exact(
                fields,
                &["kind", "invocationId", "byteLength", "chunkCount"],
            )?;
            let byte_length = count(&fields["byteLength"], "byteLength")?;
            let chunk_count = count(&fields["chunkCount"], "chunkCount")?;
            if byte_length == 0
                || byte_length > MAX_RESULT_BYTES
                || chunk_count == 0
                || chunk_count > MAX_RESULT_CHUNKS
                || chunk_count != byte_length.div_ceil(RESULT_CHUNK_MAX_BYTES)
            {
                return Err(ProtocolError::invalid(
                    "Invalid Client Capability result bounds",
                ));
            }
            Ok(ClientFrame::ResultStart {
                invocation_id: invocation_id()?,
                byte_length,
                chunk_count,
            })
        }
        Some("client.capability.result_chunk") => {
            exact(fields, &["kind", "invocationId", "index", "data"])?;
            let data = string(
                &fields["data"],
                "data",
                (RESULT_CHUNK_MAX_BYTES.div_ceil(3) * 4) as usize,
            )?;
            // STANDARD requires padding and rejects nonzero trailing pad bits.
            if STANDARD.decode(&data).is_err() {
                return Err(ProtocolError::invalid(
                    "Invalid Client Capability result chunk",
                ));
            }
            Ok(ClientFrame::ResultChunk {
                invocation_id: invocation_id()?,
                index: count(&fields["index"], "index")?,
                data,
            })
        }
        Some("client.capability.interaction_request") => {
            exact(
                fields,
                &["kind", "invocationId", "interactionId", "request"],
            )?;
            Ok(ClientFrame::InteractionRequest {
                invocation_id: invocation_id()?,
                interaction_id: entity(&fields["interactionId"], "interactionId")?,
                request: decode_form_input(&fields["request"])?,
            })
        }
        _ => Err(ProtocolError::invalid(
            "Invalid Client Capability client frame kind",
        )),
    }
}

fn decode_admission_evidence(value: &Value) -> Result<AdmissionEvidence> {
    let fields = record(value, "Client Capability admission evidence")?;
    match fields.get("kind").and_then(Value::as_str) {
        Some("none") => {
            exact(fields, &["kind"])?;
            Ok(AdmissionEvidence::None)
        }
        Some("browser_url") => {
            exact(fields, &["kind", "url"])?;
            Ok(AdmissionEvidence::BrowserUrl {
                url: string(&fields["url"], "url", 16384)?,
            })
        }
        _ => Err(ProtocolError::invalid(
            "Unknown Client Capability admission evidence kind",
        )),
    }
}
