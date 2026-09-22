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

use super::{Binding, Definition, Descriptor, Error, Identity};
use crate::{composition::Scope, contributions::Catalog};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    #[serde(default)]
    pub scope: Scope,
    pub after: Option<String>,
    pub revision: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Entry {
    pub identity: Identity,
    pub descriptor: Descriptor,
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Page {
    Page {
        revision: u64,
        entries: Vec<Entry>,
        next: Option<String>,
    },
    RevisionChanged {
        revision: u64,
    },
}

/// Capture once; directory pagination never mixes different publication revisions.
pub fn query(catalog: &Catalog, query: Query) -> Result<Page, Error> {
    let snapshot = catalog.snapshot::<Definition>(&query.scope);
    if query.revision.is_some_and(|r| r != snapshot.revision) {
        return Ok(Page::RevisionChanged {
            revision: snapshot.revision,
        });
    }
    if query.after.is_some() && query.revision.is_none() {
        return Err(Error::Invalid("provider cursor requires a revision".into()));
    }
    if query
        .after
        .as_ref()
        .is_some_and(|name| !snapshot.entries.contains_key(name))
    {
        return Err(Error::Invalid("unknown provider cursor".into()));
    }
    let mut entries = Vec::new();
    let mut bytes = 0;
    let mut next = None;
    for (name, contribution) in snapshot.entries {
        if query.after.as_ref().is_some_and(|after| name <= *after) {
            continue;
        }
        let binding = Binding::new(name.clone(), contribution)?;
        let entry = Entry {
            identity: binding.identity().clone(),
            descriptor: binding.definition().descriptor().clone(),
        };
        let size = serde_json::to_vec(&entry)
            .map_err(|_| Error::Invalid("invalid provider descriptor".into()))?
            .len();
        if size > 512 * 1024 {
            return Err(Error::Invalid(
                "provider directory entry exceeds 512 KiB".into(),
            ));
        }
        if entries.len() >= 32 || bytes + size > 512 * 1024 {
            next = entries
                .last()
                .map(|entry: &Entry| entry.identity.name.clone());
            break;
        }
        bytes += size;
        entries.push(entry);
    }
    Ok(Page::Page {
        revision: snapshot.revision,
        entries,
        next,
    })
}
