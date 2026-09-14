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

use super::{MAX_RESULT_BYTES, entity, invalid, text};
use crate::{Operation, Result};
use maka_presentation::shell::ShellSnapshot;
use maka_runtime::terminal::TerminalSize;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_CONTROL_SEQUENCE: u64 = 9_007_199_254_740_990;
pub const MAX_ACQUIRE_BYTES: usize = 90 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControllerIdentity {
    pub session_id: String,
    #[serde(rename = "ref")]
    pub resource_ref: String,
    pub controller_id: String,
}
impl ControllerIdentity {
    pub fn validate(&self) -> Result<()> {
        entity(&self.session_id)?;
        entity(&self.controller_id)?;
        text(&self.resource_ref, 256)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PtyControl {
    Input { input: String },
    Resize { cols: u16, rows: u16 },
    InputAndResize { input: String, cols: u16, rows: u16 },
}
impl PtyControl {
    pub fn parts(&self) -> Result<(&str, Option<TerminalSize>)> {
        let (input, size) = match self {
            Self::Input { input } => (input.as_str(), None),
            Self::Resize { cols, rows } => ("", Some((*cols, *rows))),
            Self::InputAndResize { input, cols, rows } => (input.as_str(), Some((*cols, *rows))),
        };
        if !matches!(self, Self::Resize { .. }) {
            text(input, 32 * 1024)?;
        }
        Ok((
            input,
            size.map(|(cols, rows)| TerminalSize::new(cols, rows))
                .transpose()
                .map_err(invalid)?,
        ))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControllerControlInput {
    pub session_id: String,
    #[serde(rename = "ref")]
    pub resource_ref: String,
    pub controller_id: String,
    pub sequence: u64,
    pub control: PtyControl,
}
impl ControllerControlInput {
    pub fn identity(&self) -> ControllerIdentity {
        ControllerIdentity {
            session_id: self.session_id.clone(),
            resource_ref: self.resource_ref.clone(),
            controller_id: self.controller_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PtySnapshot {
    pub session_id: String,
    #[serde(rename = "ref")]
    pub resource_ref: String,
    pub sequence: u64,
    pub buffer: String,
    pub size: TerminalSize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControllerAcquireResult {
    pub controller_id: String,
    pub next_sequence: u64,
    pub pty: PtySnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControllerControlResult {
    pub controller_id: String,
    pub sequence: u64,
    pub resource: ShellSnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControllerReleaseResult {
    pub controller_id: String,
    pub released: bool,
}

pub fn is_controller(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::RuntimeResourceControllerAcquire
            | Operation::RuntimeResourceControllerControl
            | Operation::RuntimeResourceControllerRelease
    )
}

pub fn decode_controller_identity(value: &Value) -> Result<ControllerIdentity> {
    let input: ControllerIdentity = serde_json::from_value(value.clone()).map_err(invalid)?;
    input.validate()?;
    Ok(input)
}

pub fn decode_controller_control(value: &Value) -> Result<ControllerControlInput> {
    let input: ControllerControlInput = serde_json::from_value(value.clone()).map_err(invalid)?;
    input.identity().validate()?;
    sequence(input.sequence, MAX_CONTROL_SEQUENCE)?;
    input.control.parts()?;
    Ok(input)
}

pub fn validate_controller_output(operation: Operation, value: &Value) -> Result<()> {
    use Operation::*;
    match operation {
        RuntimeResourceControllerAcquire => {
            let output: ControllerAcquireResult =
                serde_json::from_value(value.clone()).map_err(invalid)?;
            entity(&output.controller_id)?;
            sequence(output.next_sequence, MAX_CONTROL_SEQUENCE + 1)?;
            entity(&output.pty.session_id)?;
            text(&output.pty.resource_ref, 256)?;
            if output.pty.sequence > MAX_CONTROL_SEQUENCE + 1 || output.pty.buffer.len() > 80 * 1024
            {
                return Err(invalid("Invalid PTY snapshot"));
            }
            bounded(value, MAX_ACQUIRE_BYTES)
        }
        RuntimeResourceControllerControl => {
            let output: ControllerControlResult =
                serde_json::from_value(value.clone()).map_err(invalid)?;
            entity(&output.controller_id)?;
            sequence(output.sequence, MAX_CONTROL_SEQUENCE)?;
            output.resource.validate().map_err(invalid)?;
            bounded(value, MAX_RESULT_BYTES)
        }
        RuntimeResourceControllerRelease => {
            let output: ControllerReleaseResult =
                serde_json::from_value(value.clone()).map_err(invalid)?;
            entity(&output.controller_id)
        }
        _ => Err(invalid("Not a controller operation")),
    }
}

fn bounded(value: &Value, limit: usize) -> Result<()> {
    if serde_json::to_vec(value).map_err(invalid)?.len() > limit {
        return Err(invalid("Controller result exceeds wire limit"));
    }
    Ok(())
}
fn sequence(value: u64, max: u64) -> Result<()> {
    if !(1..=max).contains(&value) {
        return Err(invalid("Invalid controller sequence"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn controller_limits_preserve_raw_input_and_reject_ambiguous_effects() {
        let wire = |control: Value| {
            json!({
                "sessionId": "session", "ref": "maka://runtime/background-tasks/task",
                "controllerId": "controller", "sequence": MAX_CONTROL_SEQUENCE, "control": control,
            })
        };
        for control in [
            json!({"kind":"input","input":"\u{1b}[A\0中\r"}),
            json!({"kind":"input","input":"x".repeat(32 * 1024)}),
            json!({"kind":"resize","cols":240,"rows":100}),
            json!({"kind":"input_and_resize","input":" ","cols":2,"rows":1}),
        ] {
            let value = wire(control);
            assert_eq!(
                serde_json::to_value(decode_controller_control(&value).unwrap()).unwrap(),
                value
            );
        }
        for control in [
            json!({"kind":"input","input":""}),
            json!({"kind":"input","input":"中".repeat(32 * 1024 / 3 + 1)}),
            json!({"kind":"input","input":"x","cols":80}),
            json!({"kind":"resize","cols":1,"rows":24}),
            json!({"kind":"resize","cols":80,"rows":101}),
            json!({"kind":"input_and_resize","input":"effect","cols":80.5,"rows":24}),
            json!({"kind":"input_and_resize","input":null,"cols":80,"rows":24}),
        ] {
            assert!(decode_controller_control(&wire(control)).is_err());
        }
        for sequence in [0, MAX_CONTROL_SEQUENCE + 1] {
            let mut value = wire(json!({"kind":"resize","cols":80,"rows":24}));
            value["sequence"] = json!(sequence);
            assert!(decode_controller_control(&value).is_err());
        }
        let mut acquire = json!({
            "controllerId":"controller", "nextSequence":1,
            "pty":{"sessionId":"session","ref":"ref","sequence":0,
                "buffer":"x".repeat(80 * 1024),"size":{"cols":80,"rows":24}},
        });
        assert!(
            validate_controller_output(Operation::RuntimeResourceControllerAcquire, &acquire)
                .is_ok()
        );
        acquire["pty"]["buffer"] = json!("\0".repeat(16 * 1024));
        assert!(
            validate_controller_output(Operation::RuntimeResourceControllerAcquire, &acquire)
                .is_err(),
            "the encoded envelope budget also counts JSON escaping"
        );
    }
}
