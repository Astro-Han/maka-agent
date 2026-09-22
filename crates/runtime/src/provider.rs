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

use crate::scope::Scope;
use serde::{Deserialize, Serialize};

/// Durable recipient, not a registration lease. Re-loading the same package can
/// restore it; another package, Entry or scope cannot inherit its credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub package_id: String,
    pub entry_id: String,
    pub scope: Scope,
    pub name: String,
}
impl Identity {
    pub fn validate(&self) -> Result<(), String> {
        Scope::try_from(String::from(self.scope.clone()))?;
        for value in [&self.package_id, &self.entry_id] {
            if value.len() > 128
                || !value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                || value.split(['.', '_', ':', '-']).any(|part| {
                    part.is_empty()
                        || !part
                            .bytes()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                })
            {
                return Err("invalid model provider owner".into());
            }
        }
        if self.name.is_empty()
            || self.name.len() > 256
            || self
                .name
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
            || self.scope == Scope::DesktopUi
        {
            return Err("invalid model provider identity".into());
        }
        Ok(())
    }
}
