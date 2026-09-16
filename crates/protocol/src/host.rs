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

use crate::{ProtocolError, Result, codec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod status;
pub use status::{Diagnostics, Platform, Residency, Status, decode_diagnostics, decode_status};

/// Maintenance targets a process lifetime, not merely a State Root or a PID.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetirementInput {
    pub expected_host_epoch: String,
    pub allow_interrupt_active_tasks: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_cooperative_handoff: Option<bool>,
    /// A trusted operator may coordinate retirement with this still-connected client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handoff_connection_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RetirementResult {
    ActiveTasks,
    Prepared { pid: std::num::NonZeroU32 },
}

pub fn decode_retirement_input(value: &Value) -> Result<RetirementInput> {
    codec::shaped(
        codec::record(value, "Host retirement")?,
        &["expectedHostEpoch", "allowInterruptActiveTasks"],
        &["allowCooperativeHandoff", "handoffConnectionId"],
    )?;
    let boolean = |value: &Value| {
        value
            .as_bool()
            .ok_or_else(|| ProtocolError::invalid("Invalid retirement policy"))
    };
    Ok(RetirementInput {
        expected_host_epoch: codec::string(&value["expectedHostEpoch"], "Host epoch", 128)?,
        allow_interrupt_active_tasks: boolean(&value["allowInterruptActiveTasks"])?,
        allow_cooperative_handoff: value
            .get("allowCooperativeHandoff")
            .map(boolean)
            .transpose()?,
        handoff_connection_id: value
            .get("handoffConnectionId")
            .map(|value| codec::string(value, "Handoff connection ID", 128))
            .transpose()?,
    })
}

pub fn decode_retirement_result(value: &Value) -> Result<RetirementResult> {
    let fields = codec::record(value, "Host retirement result")?;
    match value["kind"].as_str() {
        Some("active_tasks") => {
            codec::exact(fields, &["kind"])?;
            Ok(RetirementResult::ActiveTasks)
        }
        Some("prepared") => {
            codec::exact(fields, &["kind", "pid"])?;
            let pid = u32::try_from(codec::count(&value["pid"], "Host PID")?)
                .ok()
                .and_then(std::num::NonZeroU32::new)
                .ok_or_else(|| ProtocolError::invalid("Invalid Host PID"))?;
            Ok(RetirementResult::Prepared { pid })
        }
        _ => Err(ProtocolError::invalid("Invalid retirement result")),
    }
}
