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

use maka_protocol::OperationErrorCode;
use maka_protocol::codec::{count, exact, record, string};
use maka_protocol::{COMPOSITION_ID, Operation, OperationRegistry, ProtocolError, Result};
use serde_json::{Value, json};

pub(super) struct Operations;

impl OperationRegistry for Operations {
    fn decode_input(&self, operation: Operation, value: &Value) -> Result<Value> {
        if operation != Operation::HostStatus {
            return Err(ProtocolError::invalid("Unknown operation"));
        }
        exact(record(value, "host.status input")?, &[])?;
        Ok(json!({}))
    }
    fn decode_output(&self, operation: Operation, value: &Value) -> Result<Value> {
        if operation != Operation::HostStatus {
            return Err(ProtocolError::invalid("Unknown operation"));
        }
        exact(
            record(value, "host.status output")?,
            &[
                "hostEpoch",
                "compositionId",
                "compositionRevision",
                "state",
                "connections",
                "activeOperations",
                "activeResidencies",
            ],
        )?;
        for key in ["hostEpoch", "compositionId", "compositionRevision"] {
            string(&value[key], key, 128)?;
        }
        for key in ["connections", "activeOperations", "activeResidencies"] {
            count(&value[key], key)?;
        }
        if !matches!(
            value["state"].as_str(),
            Some("starting" | "containing" | "recovering" | "ready" | "draining")
        ) {
            return Err(ProtocolError::invalid("Invalid host lifecycle"));
        }
        Ok(value.clone())
    }
    fn error_codes(&self, operation: Operation) -> Option<&[OperationErrorCode]> {
        (operation == Operation::HostStatus).then_some(
            &[
                OperationErrorCode::HostDraining,
                OperationErrorCode::InternalFailure,
            ][..],
        )
    }
}

pub(super) fn status(
    epoch: &str,
    connections: usize,
    active: usize,
    state: maka_protocol::handshake::Lifecycle,
) -> Value {
    json!({
        "hostEpoch": epoch, "compositionId": COMPOSITION_ID, "compositionRevision": "3",
        "state": state, "connections": connections, "activeOperations": active, "activeResidencies": active,
    })
}
