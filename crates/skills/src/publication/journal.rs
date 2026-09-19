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

use super::{
    Error, io,
    tree::{Fact, Manifest, Tree},
    validate_id,
};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub schema: u32,
    pub id: String,
    pub expected: Option<Manifest>,
    pub next: Option<Manifest>,
}
impl Intent {
    pub fn validate(&self) -> Result<(), Error> {
        validate_id(&self.id)?;
        if self.schema != 1 || (self.expected.is_none() && self.next.is_none()) {
            return Err(Error::Invalid("Unsupported publication intent".into()));
        }
        for manifest in [&self.expected, &self.next].into_iter().flatten() {
            if manifest.len() > 256 {
                return Err(Error::Invalid("Oversized Skill manifest".into()));
            }
            for (path, fact) in manifest {
                super::tree::validate(path)?;
                if let Fact::File { hash, mode } = fact
                    && (*mode > 0o777 || !valid_hash(hash))
                {
                    return Err(Error::Invalid("Invalid Skill artifact manifest".into()));
                }
            }
        }
        Ok(())
    }
}
pub(super) fn replay(
    skills: &Dir,
    transaction: &Dir,
    intent: &Intent,
    hash: &str,
) -> Result<(), Error> {
    for entry in transaction.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let directory = matches!(name.to_str(), Some("old" | "next"));
        if (!directory
            && !matches!(
                name.to_str(),
                Some("intent.json" | "intent.pending" | "committed" | "committed.pending")
            ))
            || entry.file_type()?.is_symlink()
            || (directory && !entry.file_type()?.is_dir())
            || (!directory && !entry.file_type()?.is_file())
        {
            return Err(Error::Invalid(
                "Unexpected Skill transaction artifact".into(),
            ));
        }
    }
    match io::read(transaction, Path::new("committed"), 80) {
        Ok((bytes, _)) if bytes == hash.as_bytes() => return Ok(()),
        Ok(_) => return Err(Error::Invalid("Invalid Skill commit marker".into())),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut target = manifest(skills, &intent.id)?;
    let mut old = manifest(transaction, "old")?;
    let next = manifest(transaction, "next")?;
    if let Some(old) = &old
        && Some(old) != intent.expected.as_ref()
    {
        return Err(Error::Invalid("Retained Skill files changed".into()));
    }
    if let Some(next) = &next
        && Some(next) != intent.next.as_ref()
    {
        return Err(Error::Invalid("Staged Skill files changed".into()));
    }
    if old.is_none() && intent.expected.is_some() {
        if target != intent.expected {
            return Err(Error::Conflict);
        }
        io::rename(skills, &intent.id, transaction, "old")?;
        old = manifest(transaction, "old")?;
        if old != intent.expected {
            // Preserve an edit which raced takeover; never overwrite a new target.
            io::rename(transaction, "old", skills, &intent.id)?;
            return Err(Error::Conflict);
        }
        target = None;
    }
    if let Some(expected_next) = &intent.next {
        if next.is_some() {
            if target.is_some() {
                return Err(if old.is_none() {
                    Error::Conflict
                } else {
                    Error::Invalid(
                        "A new Skill target conflicts with the retained generation".into(),
                    )
                });
            }
            io::rename(transaction, "next", skills, &intent.id)?;
            if manifest(skills, &intent.id)?.as_ref() != Some(expected_next) {
                return Err(Error::Invalid("Published Skill files changed".into()));
            }
        } else if target.as_ref() != Some(expected_next) {
            return Err(Error::Invalid(
                "Skill publication has no complete next generation".into(),
            ));
        }
    } else if old.is_none() {
        return Err(Error::Invalid(
            "Skill deletion has no retained generation".into(),
        ));
    }
    // Reconfirm rename durability on recovery before recording the completed cut.
    io::sync(skills)?;
    io::sync(transaction)?;
    match transaction.remove_file("committed.pending") {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    io::write_new(
        transaction,
        Path::new("committed.pending"),
        hash.as_bytes(),
        0o600,
    )?;
    io::rename(transaction, "committed.pending", transaction, "committed")?;
    Ok(())
}
fn manifest(parent: &Dir, name: &str) -> Result<Option<Manifest>, Error> {
    match parent.open_dir_nofollow(name) {
        Ok(directory) => Ok(Some(
            Tree::read(&directory, &CancellationToken::new())?.manifest(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn valid_hash(hash: &str) -> bool {
    hash.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
