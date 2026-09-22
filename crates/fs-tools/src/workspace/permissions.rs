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

use maka_sandbox::{
    Error, Mode, Network, Sandbox,
    filesystem::{Access, Policy, Rule},
};
use std::path::{Path, PathBuf};

/// Resolve a workspace preset and its immutable metadata ceiling. Embeddings
/// must intersect both with their own private-state protections before use.
pub fn resolve(mode: Mode, cwd: &Path) -> Result<(Sandbox, Sandbox), Error> {
    if mode == Mode::DangerFullAccess {
        return Ok((Sandbox::Disabled, Sandbox::Disabled));
    }
    let mut filesystem = Policy::uniform(Access::Read);
    if mode == Mode::WorkspaceWrite {
        filesystem.rules.push(Rule::subtree(cwd, Access::Write));
        let temporary = std::env::temp_dir().canonicalize()?;
        let temporary = PathBuf::from(
            super::project::host_path(&temporary)
                .map_err(|error| Error::Invalid(error.to_string()))?,
        );
        filesystem
            .rules
            .push(Rule::subtree(temporary, Access::Write));
    }
    let base = Sandbox::Managed {
        filesystem,
        network: Network::Denied,
    };
    let mut rules = vec![
        Rule::subtree(cwd.join(".git"), Access::Read),
        Rule::subtree(cwd.join(".agents"), Access::Read),
        Rule::subtree(cwd.join(".maka"), Access::Read),
        Rule::exact(cwd.join(super::MARKER_FILE), Access::Read),
    ];
    rules.extend(
        super::git_metadata(cwd)?
            .into_iter()
            .map(|path| Rule::subtree(path, Access::Read)),
    );
    let ceiling = Sandbox::Managed {
        filesystem: Policy {
            default: Access::Write,
            rules,
            deny_globs: Vec::new(),
        },
        network: Network::Allowed,
    };
    Ok((base.intersect(&ceiling)?, ceiling))
}
