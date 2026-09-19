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

use super::SessionConfiguration;
use maka_runtime::execution::SystemPrompt;
use std::collections::BTreeSet;

impl SessionConfiguration {
    pub(crate) fn tool_ceiling(&self, other: Option<BTreeSet<String>>) -> Option<BTreeSet<String>> {
        match (&self.bound_tools, other) {
            (None, other) => other,
            (Some(own), None) => Some(own.clone()),
            (Some(own), Some(other)) => Some(own.intersection(&other).cloned().collect()),
        }
    }

    pub(crate) fn initial_prompt(
        &self,
        additional: &str,
    ) -> Result<Option<SystemPrompt>, &'static str> {
        let text = [self.instructions.as_deref().unwrap_or(""), additional]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if text.is_empty() {
            return Ok(None);
        }
        let prompt = SystemPrompt {
            text,
            policy_revision: 0,
            sources: Vec::new(),
        };
        prompt.validate()?;
        Ok(Some(prompt))
    }
}
