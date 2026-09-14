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

use super::{MAX_PAGE_CHARS, ReadError, ReadPage, ReadRequest, shell::read_archived_shell};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadText {
    content: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum TextEnvelope {
    Text { text: String },
}

/// Read verified model evidence, not raw output. Only known envelopes become text;
/// arbitrary JSON (including a ReadPage's continuation and metadata) stays intact.
impl ReadRequest {
    pub fn tool_result_page(
        &self,
        tool_name: &str,
        serialized: &str,
    ) -> Result<ReadPage, ReadError> {
        self.tool_result_page_with_budget(tool_name, serialized, MAX_PAGE_CHARS)
    }

    pub(crate) fn tool_result_page_with_budget(
        &self,
        tool_name: &str,
        serialized: &str,
        budget: usize,
    ) -> Result<ReadPage, ReadError> {
        if matches!(tool_name, "Bash" | "Shell" | "Read")
            && let Some(page) = read_archived_shell(serialized, self, budget)
        {
            return page;
        }
        let text = serde_json::from_str::<String>(serialized)
            .ok()
            .or_else(|| match serde_json::from_str::<TextEnvelope>(serialized) {
                Ok(TextEnvelope::Text { text }) => Some(text),
                Err(_) => None,
            })
            .or_else(|| {
                (tool_name == "Read")
                    .then(|| serde_json::from_str::<ReadText>(serialized).ok())
                    .flatten()
                    .map(|value| value.content)
            });
        self.page_with_budget(text.as_deref().unwrap_or(serialized), budget)
    }
}
