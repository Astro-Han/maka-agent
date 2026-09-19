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

use super::{Error, Snapshot};
use crate::api::*;
use serde::Serialize;
use std::collections::BTreeSet;

impl Snapshot {
    pub fn invocable(&self, input: &InvocableInput, cwd: &str) -> Result<InvocableResult, Error> {
        let skills = self;
        let catalog = skills.catalog();
        let preferences = match &skills.preferences {
            crate::Preferences::Available(prefs) => prefs,
            crate::Preferences::Unavailable => {
                return Err(Error::Source("Skill preferences are unavailable".into()));
            }
        };
        // A continuation is scoped to its target, capability selection and frozen
        // file contents, not merely the displayed (possibly truncated) metadata.
        let inventory: Vec<_> = skills
            .discovery
            .inventory
            .iter()
            .map(|skill| {
                (
                    &skill.location.reference,
                    &skill.content_sha256,
                    &skill.shadowed_by,
                )
            })
            .collect();
        let revision = maka_runtime::artifact::content_digest(&encode(&(
            "skill.invocable.v1",
            input.target(),
            cwd,
            inventory,
            preferences
                .iter()
                .map(|(reference, p)| (reference, p.enabled, p.pinned))
                .collect::<Vec<_>>(),
            skills.host.tools.iter().collect::<BTreeSet<_>>(),
            skills.host.capabilities.iter().collect::<BTreeSet<_>>(),
        ))?);
        let offset = match input {
            InvocableInput::Start { .. } => 0,
            InvocableInput::Continue {
                revision: expected,
                cursor,
                ..
            } => {
                if expected != &revision {
                    return Ok(InvocableResult::RevisionChanged {
                        expected_revision: expected.clone(),
                        actual_revision: revision,
                    });
                }
                decode_cursor(cursor, &revision)?
            }
        };
        let items: Vec<_> = catalog
            .available()
            .filter(|skill| {
                // An identity cannot be shortened into a different selectable skill.
                !skill.location.id.is_empty()
                    && skill.location.id.len() <= 256
                    && skill.location.reference.len() <= 512
            })
            .map(|skill| InvocableItem {
                reference: skill.location.reference.clone(),
                id: skill.location.id.clone(),
                name: bounded(&skill.document.manifest.name, 256),
                description: bounded(&skill.document.manifest.description, 4096),
            })
            .collect();
        if offset > items.len()
            || matches!(input, InvocableInput::Continue { .. }) && offset == items.len()
        {
            return Err(Error::Invalid("Invalid Skill catalog cursor".into()));
        }
        let mut selected = Vec::new();
        let mut encoded_size = 0;
        for item in &items[offset..] {
            let next_offset = offset + selected.len() + 1;
            let next_cursor = (next_offset < items.len()).then(|| cursor(&revision, next_offset));
            let envelope = InvocableResult::Page {
                revision: revision.clone(),
                items: Vec::new(),
                next_cursor,
            };
            let item_size = encode(item)?.len();
            let commas = selected.len();
            if selected.len() == MAX_ITEMS
                || encode(&envelope)?.len() + encoded_size + item_size + commas > MAX_PAGE_BYTES
            {
                if selected.is_empty() {
                    return Err(Error::Projection("Skill metadata cannot fit a page".into()));
                }
                break;
            }
            encoded_size += item_size;
            selected.push(item.clone());
        }
        let end = offset + selected.len();
        Ok(InvocableResult::Page {
            next_cursor: (end < items.len()).then(|| cursor(&revision, end)),
            revision,
            items: selected,
        })
    }
}

fn bounded(text: &str, bytes: usize) -> String {
    text[..text.floor_char_boundary(text.len().min(bytes))].into()
}
pub(super) fn cursor(revision: &str, offset: usize) -> String {
    format!("{revision}:{offset}")
}
pub(super) fn decode_cursor(value: &str, revision: &str) -> Result<usize, Error> {
    let invalid = || Error::Invalid("Invalid Skill catalog cursor".into());
    let (bound, offset) = value.rsplit_once(':').ok_or_else(invalid)?;
    if bound != revision || offset.is_empty() || !offset.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let offset = offset.parse::<usize>().map_err(|_| invalid())?;
    if value != cursor(revision, offset) {
        return Err(invalid());
    }
    Ok(offset)
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(value).map_err(Error::from)
}
