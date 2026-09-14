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

use cap_fs_ext::DirExt;
use cap_std::{ambient_authority, fs::Dir};
use std::{
    io,
    path::{Component, Path, PathBuf},
};

/// Captured directory ancestry keeps relative links and '..' attached to the
/// admitted root even if ambient pathnames are subsequently replaced.
pub(super) struct Directory {
    parents: Vec<Dir>,
    aliases: [PathBuf; 2],
}

impl Directory {
    pub(super) fn capture(root: &Path, path: &Path) -> io::Result<Self> {
        let canonical = root.canonicalize()?;
        let directory = Dir::open_ambient_dir(&canonical, ambient_authority())?;
        Self {
            parents: vec![directory],
            aliases: [canonical, root.into()],
        }
        .walk(path, false)
    }

    pub(super) fn dir(&self) -> &Dir {
        self.parents.last().expect("captured root remains present")
    }

    pub(super) fn child(&self, path: &Path) -> io::Result<Self> {
        let parents = self
            .parents
            .iter()
            .map(Dir::try_clone)
            .collect::<io::Result<_>>()?;
        Self {
            parents,
            aliases: self.aliases.clone(),
        }
        .walk(path, true)
    }

    fn walk(mut self, path: &Path, follow_final: bool) -> io::Result<Self> {
        let mut pending = without_dots(path);
        let mut links = 0;
        while !pending.as_os_str().is_empty() {
            let mut components = pending.components();
            let component = components.next().ok_or_else(blocked)?;
            let remaining: PathBuf = components
                .filter(|c| !matches!(c, Component::CurDir))
                .collect();
            match component {
                Component::Normal(name) => {
                    let kind = self.dir().symlink_metadata(name)?.file_type();
                    if kind.is_symlink() {
                        if !follow_final && remaining.as_os_str().is_empty() {
                            return Err(blocked());
                        }
                        links += 1;
                        if links > 40 {
                            return Err(blocked());
                        }
                        let target = self.dir().read_link_contents(name)?;
                        let target = if target.is_absolute() {
                            let relative = self
                                .aliases
                                .iter()
                                .find_map(|root| target.strip_prefix(root).ok())
                                .ok_or_else(blocked)?
                                .to_owned();
                            self.parents.truncate(1);
                            relative
                        } else {
                            target
                        };
                        pending = without_dots(&target.join(remaining));
                        continue;
                    }
                    self.parents.push(self.dir().open_dir_nofollow(name)?);
                }
                Component::ParentDir if self.parents.len() > 1 => {
                    self.parents.pop();
                }
                _ => return Err(blocked()),
            }
            pending = remaining;
        }
        Ok(self)
    }
}

fn without_dots(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}
fn blocked() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "skill path is outside its admitted directory",
    )
}
