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

use crate::{RunError, RunInput};
use maka_model::prompt::Message;
use maka_plugins::prompt::Resolved;
use maka_runtime::composition::{FrozenComposition, RequestComposition};
use maka_tools::RequestTools;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) struct Surface {
    prompt: Resolved,
    pub evidence: Arc<FrozenComposition>,
}

impl Surface {
    pub async fn capture(
        tools: &RequestTools<'_>,
        input: &RunInput,
        cancellation: &CancellationToken,
    ) -> Result<Self, RunError> {
        let prompt = tools
            .prompt(
                input
                    .configuration
                    .system_prompt
                    .as_ref()
                    .map(|prompt| prompt.text.as_str()),
                input.invocation.clone(),
                cancellation.clone(),
            )
            .await?;
        let evidence = RequestComposition {
            system_prompt: prompt.system.clone(),
            dynamic_context: prompt.contexts.clone(),
            tool_catalog_digest: tools.catalog_digest().into(),
            tools: tools.definitions(),
            provider_options: Some(input.provider_options.clone()),
            max_output_tokens: input.main_output_limit,
            sources: prompt.sources.clone(),
        }
        .freeze()
        .map_err(|error| RunError::Internal(error.into()))?;
        Ok(Self {
            prompt,
            evidence: Arc::new(evidence),
        })
    }

    pub fn apply(&self, mut history: Vec<Message>) -> Vec<Message> {
        if matches!(history.first(), Some(Message::System { .. })) {
            history.remove(0);
        }
        if let Some(system) = &self.prompt.system {
            history.insert(
                0,
                Message::System {
                    content: system.clone(),
                    provider_options: None,
                },
            );
        }
        history.extend(self.prompt.contexts.iter().cloned().map(Message::user));
        history
    }
}
