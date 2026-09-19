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

//! Governance re-reads raw artifacts through the same containment rules as discovery.
use super::{SkillLocation, directory::Directory, source::read_file};
use crate::plugin::Error;
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// This is a current path projection for an OS open action, not a file capability.
/// Subsequent file operations must still enforce their own containment checks.
pub(crate) fn resolve_path(
    location: &SkillLocation,
    target: crate::api::PathTarget,
) -> crate::api::ResolvePathResult {
    use crate::api::{PathRejection as Rejection, PathTarget, ResolvePathResult as Result};
    let resolve = || -> std::result::Result<String, Rejection> {
        let root = location
            .discovery_root
            .canonicalize()
            .map_err(|_| Rejection::Missing)?;
        let directory = location
            .path
            .canonicalize()
            .map_err(|_| Rejection::Missing)?;
        if !directory.starts_with(&root) {
            return Err(Rejection::BlockedPath);
        }
        let path = match target {
            PathTarget::Directory => directory,
            PathTarget::File => directory
                .join("SKILL.md")
                .canonicalize()
                .map_err(|_| Rejection::Missing)?,
        };
        if !path.starts_with(&root) {
            return Err(Rejection::BlockedPath);
        }
        let metadata = path.metadata().map_err(|_| Rejection::Missing)?;
        match target {
            PathTarget::Directory if !metadata.is_dir() => return Err(Rejection::NotDirectory),
            PathTarget::File if !metadata.is_file() => return Err(Rejection::NotFile),
            _ => {}
        }
        path.to_str()
            .filter(|path| path.len() <= 4096)
            .map(str::to_owned)
            .ok_or(Rejection::BlockedPath)
    };
    match resolve() {
        Ok(path) => Result::Resolved { path, target },
        Err(reason) => Result::Rejected { reason },
    }
}

pub(crate) struct Artifacts {
    pub content: Option<Vec<u8>>,
    pub baseline: Option<Vec<u8>>,
}

pub(crate) fn read(
    location: &SkillLocation,
    baseline: bool,
    cancellation: &CancellationToken,
) -> Result<Artifacts, Error> {
    let relative = location
        .path
        .strip_prefix(&location.discovery_root)
        .map_err(|error| Error::Source(error.to_string()))?;
    let parent = relative
        .parent()
        .ok_or_else(|| Error::Source("Missing Skill parent".into()))?;
    let name = relative
        .file_name()
        .ok_or_else(|| Error::Source("Missing Skill name".into()))?;
    let directory = Directory::capture(&location.discovery_root, parent)
        .and_then(|directory| directory.child(Path::new(name)))
        .map_err(|error| Error::Source(error.to_string()))?;
    let content = read_file(&directory, "SKILL.md", 1024 * 1024, cancellation).map_err(failure)?;
    let baseline = if baseline {
        match directory.child(Path::new(".maka/baseline")) {
            Ok(directory) => {
                read_file(&directory, "SKILL.md", 1024 * 1024, cancellation).map_err(failure)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(Error::Source(error.to_string())),
        }
    } else {
        None
    };
    Ok(Artifacts { content, baseline })
}
fn failure(error: super::source::ReadError) -> Error {
    match error {
        super::source::ReadError::Cancelled => Error::Retired,
        super::source::ReadError::Failure(reason) => Error::Source(format!("{reason:?}")),
    }
}
