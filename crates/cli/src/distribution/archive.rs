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

use super::{decode_integrity, regular_file, target::Target};
use flate2::read::GzDecoder;
use maka_event_log::root::private_directory;
use maka_runtime_host::server::HostError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};
use tempfile::NamedTempFile;

const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 1100 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
    os: Vec<String>,
    cpu: Vec<String>,
    #[serde(default)]
    libc: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    target: Target,
    version: String,
    integrity: String,
    files: BTreeMap<String, Member>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Member {
    size: u64,
    sha256: String,
}

fn allowed(target: Target, name: &str) -> bool {
    matches!(
        name,
        "package.json"
            | "LICENSE"
            | "NOTICE"
            | "THIRD_PARTY_NOTICES.txt"
            | "README.md"
            | "README.zh-CN.md"
    ) || name == target.executable()
        || Some(name) == target.service_executable()
}

fn required(target: Target) -> impl Iterator<Item = &'static str> {
    [
        "package.json",
        "LICENSE",
        "NOTICE",
        "THIRD_PARTY_NOTICES.txt",
        target.executable(),
    ]
    .into_iter()
    .chain(target.service_executable())
}

pub(super) fn cached(
    directory: &Path,
    target: Target,
    version: &str,
) -> Result<Option<String>, HostError> {
    match directory.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if !metadata.is_dir() => {
            return Err("native package cache entry is not a directory".into());
        }
        Ok(_) => {}
    }
    private_directory(directory)?;
    private_directory(&directory.join("bin"))?;
    let receipt: Receipt =
        serde_json::from_reader(regular_file(&directory.join("receipt.json"), 16 * 1024)?)?;
    decode_integrity(&receipt.integrity)?;
    if receipt.target != target
        || receipt.version != version
        || receipt.files.len() > 8
        || required(target).any(|name| !receipt.files.contains_key(name))
    {
        return Err("native package cache identity or file set changed".into());
    }
    for (name, expected) in &receipt.files {
        if !allowed(target, name) {
            return Err("native package cache contains an unexpected member".into());
        }
        let file = regular_file(&directory.join(name), MAX_FILE_BYTES)?;
        if digest(file)? != *expected {
            return Err("native package cache integrity mismatch".into());
        }
    }
    validate_package(directory, target, version)?;
    Ok(Some(receipt.integrity))
}

pub(super) fn publish(
    archive: NamedTempFile,
    cache: &Path,
    destination: &Path,
    target: Target,
    version: &str,
    integrity: &str,
) -> Result<(), HostError> {
    let stage = tempfile::Builder::new()
        .prefix(".native-")
        .tempdir_in(cache)?;
    // On Windows a generic temp directory may have the Administrators group
    // as owner. Create the published child with the account SID atomically;
    // never adopt the temp directory by weakening private_directory's checks.
    let package = stage.path().join("package");
    private_directory(&package)?;
    private_directory(&package.join("bin"))?;
    let mut files = BTreeMap::new();
    let mut unpacked = 0_u64;
    let decoder = GzDecoder::new(archive.reopen()?);
    // Raw entries prevent unbounded GNU/PAX metadata allocation. Native npm
    // packages need only short fixed paths, not general-purpose tar semantics.
    let mut tar = tar::Archive::new(decoder.take(MAX_EXPANDED_BYTES + 1));
    for (index, entry) in tar.entries()?.raw(true).enumerate() {
        if index >= 12 {
            return Err("native package contains too many archive entries".into());
        }
        let mut entry = entry?;
        let path = entry.path_bytes();
        let path = std::str::from_utf8(&path)?.to_owned();
        if entry.header().entry_type().is_dir()
            && matches!(path.as_str(), "package/" | "package/bin/")
            && entry.size() == 0
        {
            continue;
        }
        let name = path
            .strip_prefix("package/")
            .ok_or("native package member is outside package/")?;
        let executable = name == target.executable() || Some(name) == target.service_executable();
        let limit = if executable {
            MAX_FILE_BYTES
        } else {
            MAX_TEXT_BYTES
        };
        if !allowed(target, name)
            || !entry.header().entry_type().is_file()
            || files.contains_key(name)
            || entry.size() == 0
            || entry.size() > limit
        {
            return Err(
                "native package contains an invalid, duplicate, or oversized member".into(),
            );
        }
        unpacked = unpacked
            .checked_add(entry.size())
            .ok_or("native package size overflow")?;
        if unpacked > MAX_EXPANDED_BYTES {
            return Err("native package exceeds the expanded size limit".into());
        }
        // Never use a tar member as an unrestricted filesystem path, restore
        // ownership/mode bits, follow links, or run package lifecycle scripts.
        let path = package.join(name);
        let mut file = File::create_new(&path)?;
        let size = std::io::copy(&mut entry, &mut file)?;
        if size != entry.size() {
            return Err("native package member is incomplete".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(if executable {
                0o500
            } else {
                0o400
            }))?;
        }
        file.sync_all()?;
        files.insert(name.to_owned(), digest(File::open(path)?)?);
    }
    if required(target).any(|name| !files.contains_key(name)) {
        return Err("native package is missing an executable or license document".into());
    }
    // Read through the gzip trailer so a truncated stream cannot be published.
    let mut tail = tar.into_inner();
    let trailing = std::io::copy(&mut tail, &mut std::io::sink())?;
    if tail.limit() == 0 || unpacked.saturating_add(trailing) > MAX_EXPANDED_BYTES {
        return Err("native package exceeds the expanded size limit".into());
    }
    validate_package(&package, target, version)?;
    let receipt = Receipt {
        target,
        version: version.into(),
        integrity: integrity.into(),
        files,
    };
    let mut file = File::create_new(package.join("receipt.json"))?;
    file.write_all(&serde_json::to_vec(&receipt)?)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    {
        File::open(package.join("bin"))?.sync_all()?;
        File::open(&package)?.sync_all()?;
    }
    match fs::rename(&package, destination) {
        Ok(()) => {}
        Err(error) => {
            // A simultaneous download may have published first. Never replace
            // its files, and never accept an unverified/incomplete winner.
            match cached(destination, target, version)? {
                Some(existing) if existing == integrity => return Ok(()),
                _ => return Err(error.into()),
            }
        }
    }
    #[cfg(unix)]
    File::open(cache)?.sync_all()?;
    Ok(())
}

fn validate_package(directory: &Path, target: Target, version: &str) -> Result<(), HostError> {
    let package: Package =
        serde_json::from_reader(regular_file(&directory.join("package.json"), 64 * 1024)?)?;
    if package.name != target.package_name()
        || package.version != version
        || package.os != [target.os()]
        || package.cpu != [target.cpu()]
        || if target.os() == "linux" {
            package.libc != ["glibc"]
        } else {
            !package.libc.is_empty()
        }
    {
        return Err(
            "native package manifest does not match the requested target and version".into(),
        );
    }
    target.validate_binary(
        &mut regular_file(&directory.join(target.executable()), MAX_FILE_BYTES)?,
        false,
    )?;
    if let Some(service) = target.service_executable() {
        target.validate_binary(
            &mut regular_file(&directory.join(service), MAX_FILE_BYTES)?,
            true,
        )?;
    }
    Ok(())
}

fn digest(mut file: File) -> Result<Member, HostError> {
    let mut hash = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > MAX_FILE_BYTES {
            return Err("native package member exceeds size limit".into());
        }
        hash.update(&buffer[..read]);
    }
    Ok(Member {
        size,
        sha256: format!("{:x}", hash.finalize()),
    })
}
