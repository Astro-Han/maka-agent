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

use maka_client::ProviderDirectory;
use maka_protocol::model_provider::{Entry, Identity};

#[derive(Default)]
pub struct Providers {
    generation: u64,
    state: State,
}

#[derive(Default)]
enum State {
    #[default]
    Unloaded,
    Loading,
    Ready(ProviderDirectory),
    Failed,
}

impl Providers {
    pub fn refresh(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.state = State::Unloaded;
    }

    pub fn failed(&self) -> bool {
        matches!(self.state, State::Failed)
    }

    pub fn loading(&self) -> bool {
        matches!(self.state, State::Unloaded | State::Loading)
    }

    pub fn query(&mut self) -> Option<u64> {
        if !matches!(self.state, State::Unloaded) {
            return None;
        }
        self.state = State::Loading;
        Some(self.generation)
    }

    pub fn complete(&mut self, generation: u64, result: Result<ProviderDirectory, String>) {
        if generation == self.generation && matches!(self.state, State::Loading) {
            self.state = result.map_or(State::Failed, State::Ready);
        }
    }

    pub fn entries(&self) -> &[Entry] {
        match &self.state {
            State::Ready(directory) => &directory.entries,
            _ => &[],
        }
    }

    pub fn find(&self, identity: &Identity) -> Option<&Entry> {
        self.entries()
            .iter()
            .find(|entry| entry.identity == *identity)
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use maka_protocol::model_provider::{Descriptor, Method, Scope};
    use serde_json::json;

    pub fn entry(name: &str, interactive: bool) -> Entry {
        Entry {
            identity: Identity {
                package_id: "fixture.models".into(),
                entry_id: "fixture-models".into(),
                scope: Scope::Profile,
                name: name.into(),
            },
            descriptor: Descriptor {
                label: name.into(),
                configuration_schema: json!({"type":"object"}),
                configuration_defaults: json!({"baseUrl":"https://fixture.example/v1"}),
                authentication: vec![Method {
                    id: if interactive { "browser" } else { "key" }.into(),
                    label: if interactive { "Browser" } else { "API key" }.into(),
                    interactive,
                    input_schema: if interactive {
                        json!({"type":"object","properties":{},"additionalProperties":false})
                    } else {
                        json!({"type":"object","properties":{"apiKey":{"type":"string"}},"required":["apiKey"],"additionalProperties":false})
                    },
                }],
                anonymous: !interactive,
                discovery: true,
            },
        }
    }

    pub fn catalog() -> Providers {
        Providers {
            generation: 0,
            state: State::Ready(ProviderDirectory {
                revision: 1,
                entries: vec![
                    entry("openai-codex", true),
                    entry("github-copilot", true),
                    entry("xai-oauth", true),
                    entry("openai-compatible", false),
                ],
            }),
        }
    }
}
