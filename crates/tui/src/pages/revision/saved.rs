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

use super::draft::{self, Input};
use crate::editor::saved::Cursor;
use maka_protocol::{
    Operation,
    session::{copy, sources},
    turn::TurnBatchStartInput,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct View {
    pub selected: usize,
    pub display: bool,
    pub positions: Vec<Position>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Position {
    pub input: usize,
    pub display: bool,
    pub cursor: Cursor,
}
impl Default for View {
    fn default() -> Self {
        Self {
            selected: 0,
            display: false,
            positions: vec![Position::default()],
        }
    }
}
impl View {
    fn validate(&self, inputs: &[Input]) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        if self.positions.len() > inputs.len() * 2 {
            return Err("Invalid revision positions".into());
        }
        for position in &self.positions {
            if !seen.insert((position.input, position.display)) {
                return Err("Duplicate revision position".into());
            }
            let input = inputs.get(position.input).ok_or("Unknown revision input")?;
            let text = if position.display {
                input
                    .content
                    .display_text
                    .as_deref()
                    .ok_or("Missing display text")?
            } else {
                &input.content.text
            };
            position.cursor.validate(text)?;
        }
        if !seen.contains(&(self.selected, self.display)) {
            return Err("Missing selected revision input".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Stage {
    Draft,
    Copy,
    Batch,
    Abandon,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub(super) root: String,
    pub(super) origin_epoch: String,
    pub(super) copy: copy::Input,
    pub(super) turn_id: String,
    pub(super) inputs: Vec<Input>,
    pub(super) stage: Stage,
    pub(super) batch: Option<TurnBatchStartInput>,
    pub(super) view: View,
}
impl Checkpoint {
    pub fn validate(&self, root: &str) -> Result<(), String> {
        let copy::Purpose::Revision { turn_id } = &self.copy.purpose else {
            return Err("Invalid revision purpose".into());
        };
        if self.root != root || self.origin_epoch.is_empty() {
            return Err("Invalid revision Root".into());
        }
        copy::decode_input(
            Operation::SessionRevisionCreate,
            &serde_json::json!({
                "sourceSessionId": self.copy.source_session_id,
                "targetSessionId": self.copy.target_session_id,
                "expectedSourceRevision": self.copy.expected_source_revision,
                "sourceTurnId": turn_id,
            }),
        )
        .map_err(|e| e.to_string())?;
        maka_protocol::turn::decode_turn_query_input(&serde_json::json!({
            "sessionId":self.copy.target_session_id, "turnId":self.turn_id,
        }))
        .map_err(|e| e.to_string())?;
        sources::decode_output(&serde_json::json!({
            "sessionId":self.copy.source_session_id, "turnId":turn_id,
            "messages":self.inputs.iter().map(|input| &input.original).collect::<Vec<_>>(),
        }))
        .map_err(|e| e.to_string())?;
        for input in &self.inputs {
            input.validate()?;
        }
        self.view.validate(&self.inputs)?;
        if (self.stage == Stage::Batch && self.batch.is_none())
            || (matches!(self.stage, Stage::Draft | Stage::Copy) && self.batch.is_some())
            || serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > 2 * 1024 * 1024
        {
            return Err("Invalid revision checkpoint".into());
        }
        if let Some(batch) = &self.batch {
            let decoded = maka_protocol::turn::decode_turn_batch_start_input(
                &serde_json::to_value(batch).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            if decoded != *batch
                || batch.session_id != self.copy.target_session_id
                || batch.turn_id != self.turn_id
                || batch.messages.len() != self.inputs.len()
            {
                return Err("Invalid frozen revision".into());
            }
            let target = sources::Output {
                session_id: batch.session_id.clone(),
                turn_id: turn_id.clone(),
                messages: self
                    .inputs
                    .iter()
                    .zip(&batch.messages)
                    .map(|(input, message)| {
                        let mut source = input.original.clone();
                        source.content.attachments = message.content.attachments.clone();
                        source
                    })
                    .collect(),
            };
            if draft::batch(&self.inputs, &target, &self.turn_id)? != *batch {
                return Err("Frozen revision changed its inputs".into());
            }
        }
        Ok(())
    }
}
