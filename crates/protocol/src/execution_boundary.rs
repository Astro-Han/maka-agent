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

//! Bounded execution-policy projection; this wire does not report OS sandbox availability.
use crate::{ProtocolError, Result, codec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedAccess {
    ReadOnly,
    Writable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionBoundarySummary {
    Managed {
        access: ManagedAccess,
        revision: u64,
    },
    Bypass {
        revision: u64,
    },
    External {
        revision: u64,
    },
}

pub fn decode_input(value: &Value) -> Result<String> {
    let input = codec::record(value, "Session execution boundary query")?;
    codec::exact(input, &["sessionId"])?;
    let id = codec::string(&input["sessionId"], "sessionId", 128)?;
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(ProtocolError::invalid("Invalid sessionId"));
    }
    Ok(id)
}

pub fn decode_output(value: &Value) -> Result<ExecutionBoundarySummary> {
    let record = codec::record(value, "Session execution boundary summary")?;
    let revision = codec::count(
        record.get("revision").unwrap_or(&Value::Null),
        "boundary revision",
    )?;
    let mut value = value.clone();
    value["revision"] = revision.into();
    serde_json::from_value(value).map_err(|e| ProtocolError::invalid(e.to_string()))
}
