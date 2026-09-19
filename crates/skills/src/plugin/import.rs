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
use super::{Error, Skills, catalog};
use crate::{
    api::{
        ImportRejection, ImportSourceInput, ImportSourceResult, ImportedSource, ManagedSourceType,
    },
    publication::{self, Tree, UserStore},
};
use std::path::Path;
use tokio_util::sync::CancellationToken;

impl Skills {
    pub async fn import_source(
        &self,
        input: ImportSourceInput,
    ) -> Result<ImportSourceResult, Error> {
        let admitted = self.basis.owner.admit().map_err(|_| Error::Retired)?;
        let skills = self.clone();
        let receiver = self
            .basis
            .owner
            .spawn_resource("Skills source import", move |_| async move {
                let _admitted = admitted;
                let _serial = skills.mutations.write().await;
                let _invalidation = skills.input_revision.invalidate().await;
                let _notice = skills.notify_on_exit();
                let Some(home) = skills.home.clone() else {
                    return Ok(Err(Error::Source(
                        "Managed Skills require a user home".into(),
                    )));
                };
                let cancellation = skills.basis.owner.stopping().map_err(|e| e.to_string())?;
                let result = skills
                    .data
                    .run(move |_| import(&home, Path::new(&input.source_path), &cancellation))
                    .await
                    .map_err(|e| e.to_string())?;
                if let Err(Error::OutcomeUnknown(message)) = &result {
                    skills.basis.owner.cleanup_failed(message.clone());
                }
                Ok(result)
            })
            .map_err(|_| Error::Retired)?;
        receiver
            .await
            .map_err(|e| Error::OutcomeUnknown(e.to_string()))?
            .map_err(Error::OutcomeUnknown)?
    }
}

fn import(
    home: &Path,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<ImportSourceResult, Error> {
    use ImportRejection as Rejection;
    let rejected = |reason| Ok(ImportSourceResult::Rejected { reason });
    let Some(id) = source_id(path) else {
        return rejected(Rejection::InvalidSkill);
    };
    let bytes = match publication::read_import(path) {
        Ok(bytes) => bytes,
        Err(_) => return rejected(Rejection::BlockedPath),
    };
    let Ok(content) = std::str::from_utf8(&bytes) else {
        return rejected(Rejection::InvalidSkill);
    };
    let Ok(document) = crate::parse(content) else {
        return rejected(Rejection::InvalidSkill);
    };
    let source = ImportedSource {
        id: id.clone(),
        name: catalog::bounded(&document.manifest.name, 256).0,
        description: catalog::bounded(&document.manifest.description, 4096).0,
        category: catalog::managed_category(document.manifest.attributes.category.as_deref())
            .into(),
        source_type: ManagedSourceType::Local,
    };
    let publisher = UserStore::ManagedSources
        .open(home)
        .map_err(|e| Error::Source(e.to_string()))?;
    let mut tree = Tree::empty();
    tree.insert("SKILL.md", bytes)
        .map_err(|e| Error::Source(e.to_string()))?;
    match publisher.publish(&id, None, Some(&tree), cancellation) {
        Ok(()) => Ok(ImportSourceResult::Imported { source }),
        Err(publication::Error::Conflict) => rejected(Rejection::AlreadyExists),
        Err(publication::Error::Cancelled) => Err(Error::Retired),
        Err(publication::Error::OutcomeUnknown(message)) => Err(Error::OutcomeUnknown(message)),
        Err(e) => Err(Error::Source(e.to_string())),
    }
}
fn source_id(path: &Path) -> Option<String> {
    let stem = if path.file_name()?.to_str()?.eq_ignore_ascii_case("SKILL.md") {
        path.parent()?.file_name()?.to_str()?
    } else {
        path.file_stem()?.to_str()?
    };
    let mut id = String::new();
    for character in stem.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            id.push(character.to_ascii_lowercase());
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let id = &id[..id.len().min(80)];
    crate::safe_source_id(id).then(|| id.into())
}
