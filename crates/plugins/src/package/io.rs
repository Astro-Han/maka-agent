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

use super::{MAX_FILE_BYTES, MAX_FILES, MAX_PACKAGE_BYTES, Package, validate_path};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::Path,
};

#[derive(Debug, thiserror::Error)]
pub enum PackageIoError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Invalid(#[from] crate::Error),
}

impl Package {
    /// Blocking I/O; call on the Host blocking pool. Directory-relative handles
    /// refuse substituted symlinks at every package component on all platforms.
    pub fn read_from(source: &Path) -> Result<Self, PackageIoError> {
        if !source.is_absolute() {
            return Err(invalid("package source must be absolute"));
        }
        let parent = source
            .parent()
            .ok_or_else(|| invalid("package source needs a parent"))?;
        let name = source
            .file_name()
            .ok_or_else(|| invalid("package source needs a filename"))?;
        let parent = Dir::open_ambient_dir(parent, ambient_authority())?;
        let metadata = parent.symlink_metadata(name)?;
        if metadata.is_dir() && !metadata.is_symlink() {
            let directory = parent.open_dir_nofollow(name)?;
            let mut files = BTreeMap::new();
            let mut budget = Budget::default();
            collect(&directory, "", &mut files, &mut budget)?;
            Ok(Self::new(files)?)
        } else if metadata.is_file() && !metadata.is_symlink() {
            Ok(Self::from_bundle(&read(
                &parent,
                name.as_ref(),
                MAX_PACKAGE_BYTES * 2,
            )?)?)
        } else {
            Err(invalid(
                "package source must be a regular file or directory, not a symlink",
            ))
        }
    }

    /// An existing destination is never replaced, and a partial write is never
    /// published as a complete bundle.
    pub fn export_to(&self, target: &Path) -> Result<(), PackageIoError> {
        if !target.is_absolute() {
            return Err(invalid("bundle destination must be absolute"));
        }
        let parent = target
            .parent()
            .ok_or_else(|| invalid("bundle destination needs a parent"))?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&self.to_bundle())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(target)
            .map_err(|error| error.error)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

#[derive(Default)]
struct Budget {
    bytes: usize,
    entries: usize,
}

fn collect(
    directory: &Dir,
    prefix: &str,
    files: &mut BTreeMap<String, Vec<u8>>,
    budget: &mut Budget,
) -> Result<(), PackageIoError> {
    for entry in directory.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        budget.entries += 1;
        if budget.entries > 4096 {
            return Err(invalid("package contains too many directory entries"));
        }
        let name = name
            .to_str()
            .ok_or_else(|| invalid("package filename is not UTF-8"))?;
        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        validate_path(&path)?;
        let kind = entry.file_type()?;
        if kind.is_dir() && !kind.is_symlink() {
            let child = directory.open_dir_nofollow(name)?;
            collect(&child, &path, files, budget)?;
        } else if kind.is_file() && !kind.is_symlink() {
            if files.len() >= MAX_FILES {
                return Err(invalid("package contains too many files"));
            }
            let bytes = read(
                directory,
                Path::new(name),
                MAX_FILE_BYTES.min(MAX_PACKAGE_BYTES - budget.bytes),
            )?;
            budget.bytes += bytes.len();
            files.insert(path, bytes);
        } else {
            return Err(invalid("package contains a symlink or non-regular file"));
        }
    }
    Ok(())
}

fn read(directory: &Dir, name: &Path, limit: usize) -> Result<Vec<u8>, PackageIoError> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = directory.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(invalid("package file exceeds its limit or is not regular"));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("package file grew beyond its byte limit"));
    }
    Ok(bytes)
}

fn invalid(message: &str) -> PackageIoError {
    crate::Error::Invalid(message.into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn importing_and_exporting_preserves_bytes_without_following_links_or_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        let package_root = directory.path().join("package");
        fs::create_dir(&package_root).unwrap();
        fs::write(
            package_root.join("maka.extension.json"),
            br#"{"schemaVersion":1,"id":"example","runtime":{"entry":"index.mjs","sdkVersion":1}}"#,
        )
        .unwrap();
        fs::write(package_root.join("index.mjs"), b"export default {}").unwrap();
        let package = Package::read_from(&package_root).unwrap();
        let bundle = directory.path().join("example.maka-extension");
        package.export_to(&bundle).unwrap();
        assert_eq!(
            Package::read_from(&bundle).unwrap().digest(),
            package.digest()
        );
        assert!(package.export_to(&bundle).is_err());
        assert_eq!(
            Package::read_from(&bundle).unwrap().digest(),
            package.digest()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&bundle, package_root.join("linked")).unwrap();
            assert!(Package::read_from(&package_root).is_err());
        }
    }
}
