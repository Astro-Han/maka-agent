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

use serde::{Deserialize, Serialize};

pub mod input;

/// Shared by native PTY allocation, input validation and screen rendering.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "SizeFields")]
pub struct TerminalSize {
    cols: u16,
    rows: u16,
}

impl TerminalSize {
    pub fn new(cols: u16, rows: u16) -> Result<Self, &'static str> {
        if !(2..=240).contains(&cols) || !(1..=100).contains(&rows) {
            return Err("terminal size must be 2..240 columns and 1..100 rows");
        }
        Ok(Self { cols, rows })
    }

    pub fn cols(self) -> u16 {
        self.cols
    }

    pub fn rows(self) -> u16 {
        self.rows
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SizeFields {
    cols: u16,
    rows: u16,
}

impl TryFrom<SizeFields> for TerminalSize {
    type Error = &'static str;
    fn try_from(value: SizeFields) -> Result<Self, Self::Error> {
        Self::new(value.cols, value.rows)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalScreen {
    pub screen: String,
    pub scrollback: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_alternate_screen: Option<String>,
    pub size: TerminalSize,
    pub cursor: TerminalCursor,
    pub alternate_screen: bool,
    pub truncated: bool,
    pub input: TerminalInputModes,
}

impl TerminalScreen {
    pub fn new(size: TerminalSize) -> Self {
        Self {
            screen: String::new(),
            scrollback: String::new(),
            last_alternate_screen: None,
            size,
            cursor: TerminalCursor {
                x: 0,
                y: 0,
                visible: true,
            },
            alternate_screen: false,
            truncated: false,
            input: TerminalInputModes {
                application_cursor_keys_mode: false,
                mouse_tracking_mode: MouseTracking::None,
                mouse_encoding: MouseEncoding::Default,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalCursor {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalInputModes {
    pub application_cursor_keys_mode: bool,
    pub mouse_tracking_mode: MouseTracking,
    pub mouse_encoding: MouseEncoding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseTracking {
    None,
    X10,
    Vt200,
    Drag,
    Any,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseEncoding {
    Default,
    Sgr,
    SgrPixels,
}
