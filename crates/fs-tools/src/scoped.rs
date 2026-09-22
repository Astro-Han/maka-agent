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

use crate::{ReadLimits, ReadOutput, ReadScope, failed};
#[cfg(unix)]
use cap_std::fs::OpenOptionsExt;
use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};
use maka_runtime::{read::ReadRequest, tools::ToolError};
use maka_sandbox::filesystem::Compiled;
use std::sync::Arc;
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: usize = 2000;

mod directory;
mod path;
pub(crate) use directory::Directory;

struct Root {
    path: PathBuf,
    aliases: Vec<PathBuf>,
    dir: Dir,
}

pub(crate) struct Authority {
    cwd: PathBuf,
    scope: CapturedScope,
    policy: Option<Arc<Compiled>>,
}

pub(crate) struct Route {
    pub root: Directory,
    pub relative: PathBuf,
    pub root_path: PathBuf,
}

enum CapturedScope {
    Restricted(Vec<Root>),
    Unrestricted,
}

impl Authority {
    pub(crate) fn from_directory(
        path: PathBuf,
        dir: Dir,
        policy: Option<Arc<Compiled>>,
    ) -> Result<Self, ToolError> {
        if !path.is_absolute() {
            return Err(failed("Captured directory location must be absolute"));
        }
        Ok(Self {
            cwd: path.clone(),
            policy,
            scope: CapturedScope::Restricted(vec![Root {
                aliases: vec![path.clone()],
                path,
                dir,
            }]),
        })
    }
    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn write_display_path(&self, path: &Path) -> Result<String, ToolError> {
        let absolute = path::resolve(&self.cwd, path)?;
        let normalized: PathBuf = absolute.components().collect();
        dunce::simplified(&normalized)
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| failed("Write path must be UTF-8"))
    }

    pub(crate) fn routes(&self, path: &Path) -> Result<Vec<Route>, ToolError> {
        let absolute = path::resolve(&self.cwd, path)?;
        match &self.scope {
            CapturedScope::Unrestricted => {
                let root = absolute
                    .ancestors()
                    .last()
                    .ok_or_else(|| failed("absolute path required"))?;
                Ok(vec![Route {
                    root: Directory::new(
                        Dir::open_ambient_dir(root, ambient_authority()).map_err(io_error)?,
                        root.to_owned(),
                        self.policy.clone(),
                    ),
                    relative: absolute
                        .strip_prefix(root)
                        .map_err(|_| failed("invalid Write path"))?
                        .to_owned(),
                    root_path: root.to_owned(),
                }])
            }
            CapturedScope::Restricted(roots) => path::candidates(roots, &absolute)
                .into_iter()
                .map(|(root, relative, _)| {
                    Ok(Route {
                        root: Directory::new(
                            root.dir.try_clone().map_err(io_error)?,
                            root.path.clone(),
                            self.policy.clone(),
                        ),
                        relative: relative.to_owned(),
                        root_path: root.path.clone(),
                    })
                })
                .collect(),
        }
    }

    pub(crate) fn new(cwd: &Path, scope: ReadScope) -> Result<Self, ToolError> {
        if !cwd.is_absolute() {
            return Err(failed("Session cwd must be absolute"));
        }
        let cwd = cwd.canonicalize().map_err(io_error)?;
        if !cwd.is_dir() {
            return Err(failed("Session cwd must be a directory"));
        }
        let paths = match scope {
            ReadScope::Policy(policy) => {
                return Ok(Self {
                    cwd,
                    scope: CapturedScope::Unrestricted,
                    policy: Some(policy),
                });
            }
            ReadScope::Restricted { roots } => roots,
            ReadScope::Unrestricted => {
                return Ok(Self {
                    cwd,
                    scope: CapturedScope::Unrestricted,
                    policy: None,
                });
            }
        };
        if paths.is_empty() || paths.len() > 32 {
            return Err(failed("Read requires between 1 and 32 admitted roots"));
        }
        let mut roots: Vec<Root> = Vec::new();
        for path in paths {
            if !path.is_absolute() {
                return Err(failed("Read roots must be absolute"));
            }
            let canonical = path.canonicalize().map_err(io_error)?;
            if let Some(root) = roots.iter_mut().find(|root| root.path == canonical) {
                if !root.aliases.contains(&path) {
                    root.aliases.push(path);
                }
                continue;
            }
            let dir = Dir::open_ambient_dir(&canonical, ambient_authority()).map_err(io_error)?;
            let mut aliases = vec![canonical.clone()];
            if path != canonical {
                aliases.push(path);
            }
            roots.push(Root {
                path: canonical,
                aliases,
                dir,
            });
        }
        Ok(Self {
            cwd,
            scope: CapturedScope::Restricted(roots),
            policy: None,
        })
    }

    fn open(&self, absolute: &Path) -> Result<File, ToolError> {
        if self.policy.is_some() {
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            options.custom_flags(libc::O_NONBLOCK);
            let mut error = failed("path is outside the admitted Read roots");
            for route in self.routes(absolute)? {
                match route.root.open_with(&route.relative, &options) {
                    Ok(file) => return Ok(file),
                    Err(reason) => error = io_error(reason),
                }
            }
            return Err(error);
        }
        let roots = match &self.scope {
            CapturedScope::Restricted(roots) => roots,
            CapturedScope::Unrestricted => {
                let mut options = std::fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NONBLOCK);
                }
                return options.open(absolute).map(File::from_std).map_err(io_error);
            }
        };
        let mut options = OpenOptions::new();
        options.read(true);
        // A FIFO must not block before we can inspect the opened file type.
        #[cfg(unix)]
        options.custom_flags(libc::O_NONBLOCK);
        let mut error = failed("path is outside the admitted Read roots");
        for (root, relative, _) in path::candidates(roots, absolute) {
            if relative
                .components()
                .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
            {
                return Err(failed("invalid relative Read path"));
            }
            match root.dir.open_with(relative, &options) {
                Ok(file) => return Ok(file),
                Err(reason) => error = io_error(reason),
            }
        }
        Err(error)
    }

    pub(crate) fn read(
        &self,
        input: ReadRequest,
        limits: ReadLimits,
        cancellation: CancellationToken,
    ) -> Result<ReadOutput, ToolError> {
        check_cancelled(&cancellation)?;
        let path = Path::new(input.path());
        // Dispatch on the requested extension, including symlink aliases. Never
        // reopen the resolved pathname outside the captured directory capability.
        let image = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ["png", "jpg", "jpeg", "gif", "webp"]
                    .iter()
                    .any(|known| ext.eq_ignore_ascii_case(known))
            });
        let max_source_bytes = if image {
            MAX_IMAGE_BYTES
        } else {
            limits.max_source_bytes
        };
        // Keep parent components intact for capability resolution, so symlink/..
        // semantics cannot be changed by a lexical normalization.
        let absolute = path::resolve(&self.cwd, path)?;
        let mut file = self.open(&absolute)?;
        let metadata = file.metadata().map_err(io_error)?;
        if !metadata.is_file() {
            return Err(failed("Read supports only regular files"));
        }
        if metadata.len() > max_source_bytes as u64 {
            return Err(failed("Read source exceeds byte limit"));
        }
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            check_cancelled(&cancellation)?;
            let available = (max_source_bytes + 1 - bytes.len()).min(chunk.len());
            let count = file.read(&mut chunk[..available]).map_err(io_error)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.len() > max_source_bytes {
                return Err(failed("Read source exceeds byte limit"));
            }
        }
        check_cancelled(&cancellation)?;
        if image {
            let mime_type = match maka_runtime::attachment::sniff_binary_mime(&bytes) {
                Some(mime @ ("image/png" | "image/jpeg" | "image/gif" | "image/webp")) => mime,
                _ => {
                    return Err(failed(
                        "Read image requires PNG, JPEG, GIF, or WebP content",
                    ));
                }
            };
            let dimensions = imagesize::blob_size(&bytes)
                .map_err(|e| failed(format!("Read image dimensions unavailable: {e}")))?;
            if dimensions.width == 0
                || dimensions.height == 0
                || dimensions.width > MAX_IMAGE_DIMENSION
                || dimensions.height > MAX_IMAGE_DIMENSION
            {
                return Err(failed(
                    "Read image dimensions must be between 1 and 2000 pixels",
                ));
            }
            check_cancelled(&cancellation)?;
            return Ok(ReadOutput::Image {
                bytes,
                mime_type: mime_type.into(),
            });
        }
        let content = String::from_utf8(bytes).map_err(|_| failed("Read requires UTF-8 text"))?;
        if content.contains('\0') {
            return Err(failed("Read does not support binary files"));
        }
        let page = input.page(&content).map_err(|e| failed(e.to_string()))?;
        check_cancelled(&cancellation)?;
        Ok(ReadOutput::Text(page))
    }
}

fn check_cancelled(token: &CancellationToken) -> Result<(), ToolError> {
    if token.is_cancelled() {
        Err(failed("Read cancelled"))
    } else {
        Ok(())
    }
}

fn io_error(error: std::io::Error) -> ToolError {
    ToolError::Io {
        kind: error.kind(),
        message: format!("Read filesystem error: {error}"),
    }
}
