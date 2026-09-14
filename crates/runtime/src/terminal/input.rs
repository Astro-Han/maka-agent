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

mod key;
mod mouse;

pub use key::{Key, Modifier, Modifiers};
pub use mouse::{MouseAction, MouseButton, MouseEvent, ScrollDirection};

use super::{TerminalInputModes, TerminalSize};
use serde::Deserialize;
use serde_json::Value;

pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_INPUT_ACTIONS: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("terminal input rejected: {0}")]
pub struct InputError(pub &'static str);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InputAction {
    Text(String),
    Key { key: Key, modifiers: Modifiers },
    Mouse(MouseAction),
}

impl InputAction {
    /// Strict client/tool boundary. Provider-default normalization, when needed,
    /// belongs to the provider adapter, never to this decoder.
    pub fn parse(value: Value) -> Result<Self, InputError> {
        let raw: RawAction = serde_json::from_value(value)
            .map_err(|_| InputError("invalid action fields or values"))?;
        let action = match raw {
            RawAction::Text { text } => {
                if text.is_empty()
                    || text
                        .chars()
                        .any(|c| c <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&c))
                {
                    return Err(InputError(
                        "text must be nonempty and contain no terminal controls",
                    ));
                }
                Self::Text(text)
            }
            RawAction::Key { key, modifiers } => {
                let key = Key::parse(&key)?;
                // Reject unsupported combinations independently of live mode.
                key.encode(modifiers, false)?;
                Self::Key { key, modifiers }
            }
            RawAction::Mouse {
                event,
                x,
                y,
                button,
                direction,
                modifiers,
            } => Self::Mouse(MouseAction::new(event, x, y, button, direction, modifiers)?),
        };
        Ok(action)
    }
}

/// Encodes the entire batch before the caller can perform any native effect.
/// The caller supplies modes and size from one serialized screen cut.
pub fn encode_actions(
    actions: &[InputAction],
    modes: TerminalInputModes,
    size: TerminalSize,
) -> Result<String, InputError> {
    if actions.is_empty() || actions.len() > MAX_INPUT_ACTIONS {
        return Err(InputError("expected 1..64 input actions"));
    }
    let mut output = String::new();
    for action in actions {
        let encoded = match action {
            InputAction::Text(text) => {
                if text.is_empty()
                    || text
                        .chars()
                        .any(|c| c <= '\u{1f}' || ('\u{7f}'..='\u{9f}').contains(&c))
                {
                    return Err(InputError(
                        "text must be nonempty and contain no terminal controls",
                    ));
                }
                if text.len() > MAX_INPUT_BYTES.saturating_sub(output.len()) {
                    return Err(InputError("encoded input exceeds 64 KiB"));
                }
                output.push_str(text);
                continue;
            }
            InputAction::Key { key, modifiers } => {
                key.encode(*modifiers, modes.application_cursor_keys_mode)?
            }
            InputAction::Mouse(mouse) => mouse.encode(modes, size)?,
        };
        if encoded.len() > MAX_INPUT_BYTES.saturating_sub(output.len()) {
            return Err(InputError("encoded input exceeds 64 KiB"));
        }
        output.push_str(&encoded);
    }
    Ok(output)
}

/// Canonical requested byte count, independent of live modes or terminal size.
/// Admission still encodes again against the actual worker cut before effects.
pub fn encoded_actions_byte_len(actions: &[InputAction]) -> Result<usize, InputError> {
    if actions.is_empty() || actions.len() > MAX_INPUT_ACTIONS {
        return Err(InputError("expected 1..64 input actions"));
    }
    let mut length = 0usize;
    for action in actions {
        length += match action {
            InputAction::Text(text) => text.len(),
            InputAction::Key { key, modifiers } => key.encode(*modifiers, false)?.len(),
            InputAction::Mouse(mouse) => mouse.encode_sgr().len(),
        };
        if length > MAX_INPUT_BYTES {
            return Err(InputError("encoded input exceeds 64 KiB"));
        }
    }
    Ok(length)
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RawAction {
    Text {
        text: String,
    },
    Key {
        key: String,
        #[serde(default)]
        modifiers: Modifiers,
    },
    Mouse {
        event: mouse::RawEvent,
        #[serde(deserialize_with = "coordinate")]
        x: u64,
        #[serde(deserialize_with = "coordinate")]
        y: u64,
        #[serde(default, deserialize_with = "present")]
        button: Option<MouseButton>,
        #[serde(default, deserialize_with = "present")]
        direction: Option<ScrollDirection>,
        #[serde(default)]
        modifiers: Modifiers,
    },
}

// Missing is allowed; explicit null is not part of the strict wire contract.
fn present<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(de).map(Some)
}

fn coordinate<'de, D: serde::Deserializer<'de>>(de: D) -> Result<u64, D::Error> {
    let value = f64::deserialize(de)?;
    if !(0.0..=9_007_199_254_740_991.0).contains(&value) || value.fract() != 0.0 {
        return Err(serde::de::Error::custom(
            "expected a non-negative safe integer",
        ));
    }
    Ok(value as u64)
}
