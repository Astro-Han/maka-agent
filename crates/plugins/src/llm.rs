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

pub use maka_runtime::model::ModelGeneration;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Generate {
    pub prompt: String,
    pub system: Option<String>,
    pub max_output_tokens: Option<u64>,
}
impl Generate {
    pub fn validate(&self) -> Result<(), crate::Error> {
        if self.prompt.trim().is_empty()
            || self
                .prompt
                .len()
                .saturating_add(self.system.as_ref().map_or(0, String::len))
                > 256 * 1024
            || self
                .max_output_tokens
                .is_some_and(|value| value == 0 || value > 9_007_199_254_740_991)
        {
            return Err(crate::Error::Invalid(
                "invalid or oversized model generation".into(),
            ));
        }
        Ok(())
    }
}
