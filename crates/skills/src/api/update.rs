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

#[derive(Debug, Clone)]
pub enum UpdateConfirmation {
    UnmodifiedOnly,
    Confirmed {
        current_sha256: String,
        source_sha256: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "WireUpdate", into = "WireUpdate")]
pub struct ManagedUpdate {
    pub reference: String,
    pub confirmation: UpdateConfirmation,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireUpdate {
    #[serde(rename = "ref")]
    reference: String,
    force: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    expected_current_sha256: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    expected_source_sha256: Option<String>,
}
impl TryFrom<WireUpdate> for ManagedUpdate {
    type Error = &'static str;
    fn try_from(value: WireUpdate) -> Result<Self, Self::Error> {
        let confirmation = match (
            value.force,
            value.expected_current_sha256,
            value.expected_source_sha256,
        ) {
            (false, None, None) => UpdateConfirmation::UnmodifiedOnly,
            (true, Some(current_sha256), Some(source_sha256))
                if hash(&current_sha256) && hash(&source_sha256) =>
            {
                UpdateConfirmation::Confirmed {
                    current_sha256,
                    source_sha256,
                }
            }
            _ => return Err("Managed update requires both exact preview hashes or neither"),
        };
        Ok(Self {
            reference: value.reference,
            confirmation,
        })
    }
}
impl From<ManagedUpdate> for WireUpdate {
    fn from(value: ManagedUpdate) -> Self {
        let (force, expected_current_sha256, expected_source_sha256) = match value.confirmation {
            UpdateConfirmation::UnmodifiedOnly => (false, None, None),
            UpdateConfirmation::Confirmed {
                current_sha256,
                source_sha256,
            } => (true, Some(current_sha256), Some(source_sha256)),
        };
        Self {
            reference: value.reference,
            force,
            expected_current_sha256,
            expected_source_sha256,
        }
    }
}
fn hash(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
