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
use super::{Error, Publisher, io};
use cap_fs_ext::DirExt;
use cap_std::{ambient_authority, fs::Dir};
use std::{path::Path, sync::Arc};

/// These are domain-owned user stores, never arbitrary paths from a journal.
#[derive(Clone, Copy)]
pub(crate) enum UserStore {
    MakaSkills,
    AgentSkills,
    ManagedSources,
}
impl UserStore {
    fn paths(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::MakaSkills => (".maka", "skills", ".skills-publication"),
            Self::AgentSkills => (".agents", "skills", ".skills-publication"),
            Self::ManagedSources => (".maka", "skill-sources", ".skill-sources-publication"),
        }
    }
    pub(crate) fn open(self, home: &Path) -> Result<Publisher, Error> {
        let home = Dir::open_ambient_dir(home, ambient_authority())?;
        let (parent, skills, journal) = self.paths();
        let parent = io::child(&home, parent)?;
        let data = io::child(&parent, journal)?;
        let lock = Arc::new(io::lock(&data)?);
        // Staging and destination share a parent filesystem, even when the
        // user's home and Host private data live on different volumes.
        Ok(Publisher {
            skills: io::child(&parent, skills)?,
            transactions: io::child(&data, "transactions")?,
            _lock: lock,
        })
    }
    pub(crate) fn recover_existing(home: &Path) -> Result<(), Error> {
        let home = match Dir::open_ambient_dir(home, ambient_authority()) {
            Ok(home) => home,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        for store in [Self::MakaSkills, Self::AgentSkills, Self::ManagedSources] {
            let (parent, skills, journal) = store.paths();
            let parent = match home.open_dir_nofollow(parent) {
                Ok(parent) => parent,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let data = match parent.open_dir_nofollow(journal) {
                Ok(data) => data,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let lock = Arc::new(io::lock(&data)?);
            Publisher {
                skills: io::child(&parent, skills)?,
                transactions: io::child(&data, "transactions")?,
                _lock: lock,
            }
            .recover()?;
        }
        Ok(())
    }
}
