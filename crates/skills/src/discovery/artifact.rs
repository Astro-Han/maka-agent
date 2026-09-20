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

use super::SkillLocation;
use maka_plugins::{
    filesystem::entries::{ListFiles, ReadFile},
    filesystem::{ReadDirectory, ReadError, ReadViewInput, Symlinks},
};

/// An OS-open projection, not authority to reopen a pathname. Validate through
/// the admitted read capability; later OS actions must enforce their own boundary.
pub(crate) async fn resolve_path(
    files: &ReadDirectory,
    location: &SkillLocation,
    target: crate::api::PathTarget,
) -> crate::api::ResolvePathResult {
    use crate::api::{PathRejection as Rejection, PathTarget, ResolvePathResult as Outcome};
    let rejected = |reason| Outcome::Rejected { reason };
    if files.location() != location.discovery_root {
        return rejected(Rejection::BlockedPath);
    }
    let path = match target {
        PathTarget::Directory => location.path.clone(),
        PathTarget::File => location.path.join("SKILL.md"),
    };
    let Ok(relative) = path.strip_prefix(files.location()) else {
        return rejected(Rejection::BlockedPath);
    };
    let Some(parts) = relative
        .components()
        .map(|part| match part {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
    else {
        return rejected(Rejection::BlockedPath);
    };
    let relative = parts.join("/");
    let result = match target {
        PathTarget::Directory => files
            .list(ListFiles {
                path: relative,
                after: None,
                limit: 1,
            })
            .await
            .map(|_| ()),
        PathTarget::File => files
            .read(ReadViewInput {
                file: ReadFile {
                    path: relative,
                    offset: 0,
                    limit: 1,
                },
                symlinks: Symlinks::Reject,
            })
            .await
            .map(|_| ()),
    };
    match result {
        Ok(()) => match path.to_str().filter(|path| path.len() <= 4096) {
            Some(path) => Outcome::Resolved {
                path: path.into(),
                target,
            },
            None => rejected(Rejection::BlockedPath),
        },
        Err(ReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            rejected(Rejection::Missing)
        }
        Err(ReadError::Io(error)) if error.kind() == std::io::ErrorKind::NotADirectory => {
            rejected(Rejection::NotDirectory)
        }
        Err(_) => rejected(Rejection::BlockedPath),
    }
}
