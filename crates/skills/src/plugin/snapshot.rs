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

use super::Error;
use crate::{Catalog, HostCapabilities, Preferences, PreparedInvocation, SkillInvocationResult};
use maka_runtime::input::{InlineReferenceKind, MessageInput};
use std::collections::BTreeSet;

mod model;
pub(super) use model::registrations;

/// Immutable domain input. Host persists its result, not this executable object.
pub struct Snapshot {
    pub(super) input_basis: Option<maka_plugins::revision::Basis>,
    pub preference_revision: Option<u64>,
    pub discovery: crate::DiscoverySnapshot,
    pub preferences: Preferences,
    pub host: HostCapabilities,
    pub(super) basis: Option<maka_plugins::fiber::Context>,
}

pub enum InputPreparation {
    Ready {
        skill_invocation: SkillInvocationResult,
        required_tools: BTreeSet<String>,
    },
    Blocked(SkillInvocationResult),
}

impl Snapshot {
    /// Absence is useful for ordinary chat; explicit requests still resolve to failure.
    pub fn empty() -> Self {
        Self {
            input_basis: None,
            preference_revision: None,
            discovery: Default::default(),
            preferences: Preferences::Available(Default::default()),
            host: Default::default(),
            basis: None,
        }
    }
}

impl Snapshot {
    pub fn catalog(&self) -> Catalog<'_> {
        Catalog {
            discovery: &self.discovery,
            preferences: &self.preferences,
            host: &self.host,
        }
    }

    pub fn prepare(
        &self,
        content: &mut MessageInput,
        ids: &[String],
    ) -> Result<InputPreparation, Error> {
        match self.catalog().prepare_invocation(&content.text, ids) {
            PreparedInvocation::Passthrough => Ok(InputPreparation::Ready {
                skill_invocation: Default::default(),
                required_tools: Default::default(),
            }),
            PreparedInvocation::Blocked(result) => Ok(InputPreparation::Blocked(result)),
            PreparedInvocation::Ready {
                text,
                result,
                required_tools,
            } => {
                let display = content.display_text.clone().unwrap_or_else(|| {
                    if !content.text.trim().is_empty() {
                        content.text.clone()
                    } else {
                        result
                            .loaded
                            .iter()
                            .map(|skill| format!("/skill:{}", skill.id))
                            .collect::<Vec<_>>()
                            .join(" ")
                    }
                });
                let mut references = content.inline_references.take().unwrap_or_default();
                references.retain(|reference| reference.kind != InlineReferenceKind::Skill);
                references.extend(crate::inline_references(&result.receipts, &display));
                references.sort_by(|a, b| {
                    a.start.cmp(&b.start).then_with(|| {
                        b.value
                            .encode_utf16()
                            .count()
                            .cmp(&a.value.encode_utf16().count())
                    })
                });
                let mut end = 0;
                let references = references
                    .into_iter()
                    .filter(|reference| {
                        if reference.start < end {
                            return false;
                        }
                        end = reference
                            .start
                            .saturating_add(reference.value.encode_utf16().count() as u64);
                        true
                    })
                    .take(32)
                    .collect::<Vec<_>>();
                content.text = text;
                content.display_text = Some(display);
                content.inline_references = (!references.is_empty()).then_some(references);
                // Preparation does not silently enlarge the durable admission
                // budget or truncate instructions/user content to force success.
                if content.text_bytes() > 64 * 1024
                    || serde_json::to_vec(content)?.len() > 64 * 1024
                {
                    return Err(Error::InputTooLarge);
                }
                result
                    .validate()
                    .map_err(|error| Error::Invalid(error.into()))?;
                Ok(InputPreparation::Ready {
                    skill_invocation: result,
                    required_tools,
                })
            }
        }
    }
}
