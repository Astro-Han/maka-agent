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

//! Disposable Rust roots only. The discovery marker and both owner leases match TypeScript.

#[path = "root_lock.rs"]
mod lock;
pub use lock::{FileLease, private_directory};

#[cfg(windows)]
pub mod windows;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const ROOT_MARKER: &str = ".maka-storage-root.json";
pub const RUST_ROOT_MARKER: &str = ".maka-rust-runtime.json";
pub const ROOT_DATABASE: &str = "runtime-rust.sqlite";
const PROTOTYPE: &[u8] = b"{\"schemaVersion\":1,\"runtime\":\"rust-prototype\"}\n";

#[derive(Debug, Clone)]
pub struct RootNamespaces {
    pub ownership: PathBuf,
    pub control: PathBuf,
}

impl RootNamespaces {
    pub fn for_current_account() -> io::Result<Self> {
        let home = lock::account_home()?;
        #[cfg(target_os = "macos")]
        return Ok(Self {
            ownership: home.join("Library/Application Support/Maka/state-root-owners"),
            control: home.join("Library/Caches/Maka/runtime-hosts"),
        });
        #[cfg(target_os = "linux")]
        return Ok(Self {
            ownership: home.join(".local/share/Maka/state-root-owners"),
            control: home.join(".cache/maka/runtime-hosts"),
        });
        #[cfg(windows)]
        return Ok(Self {
            ownership: home.join("AppData/Local/Maka/state-root-owners"),
            control: home.join("AppData/Local/Maka/runtime-hosts"),
        });
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Marker {
    schema_version: u8,
    kind: String,
    root_id: String,
    root_identity: Identity,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Identity {
    dev: String,
    ino: String,
}

/// Dropping this value releases both OS leases. It cannot be cloned or fabricated.
pub struct RootOwner {
    canonical_path: PathBuf,
    marker: Marker,
    control_directory: PathBuf,
    lock_path: PathBuf,
    durable_lease: File,
    compatibility_lease: File,
}

/// A verified location, not a writer lease. Every mutation still needs RootOwner.
#[derive(Debug, Clone)]
pub struct RootLocation {
    canonical_path: PathBuf,
    root_id: String,
}

impl RootLocation {
    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub fn root_id(&self) -> &str {
        &self.root_id
    }
}

/// Inspect only an existing native root. Never initialize, repair or acquire it.
pub fn resolve(path: &Path) -> io::Result<RootLocation> {
    let (canonical_path, marker) = inspect(path)?;
    Ok(RootLocation {
        canonical_path,
        root_id: marker.root_id,
    })
}

fn inspect(path: &Path) -> io::Result<(PathBuf, Marker)> {
    let canonical_path = path.canonicalize()?;
    let identity = directory_identity(&canonical_path)?;
    check_layout(&canonical_path)?;
    let marker = read_marker(&canonical_path)?;
    if marker.root_identity != identity
        || directory_identity(&canonical_path)? != identity
        || canonical_path.canonicalize()? != canonical_path
    {
        return Err(io::Error::other("root identity collision"));
    }
    Ok((canonical_path, marker))
}

/// Verify an existing native root without taking its writer lease, or initialize an empty one.
pub fn initialize(path: &Path, namespaces: &RootNamespaces) -> io::Result<String> {
    if !path.join(ROOT_MARKER).exists() {
        return Ok(RootOwner::create(path, namespaces)?.root_id().to_owned());
    }
    Ok(resolve(path)?.root_id)
}

impl RootOwner {
    /// Initialize a nonexistent or empty disposable directory. Never adopt existing state.
    pub fn create(path: &Path, namespaces: &RootNamespaces) -> io::Result<Self> {
        #[cfg(not(windows))]
        fs::create_dir_all(path)?;
        #[cfg(windows)]
        if !path.exists() {
            lock::private_directory(path)?;
        }
        let path = path.canonicalize()?;
        if fs::read_dir(&path)?.next().is_some() {
            return Err(io::Error::other(
                "refusing to initialize a nonempty State Root",
            ));
        }
        lock::private_directory(&path)?;
        let identity = directory_identity(&path)?;
        let marker = Marker {
            schema_version: 1,
            kind: "interactive".into(),
            root_id: format!("{:x}", Sha256::digest(uuid::Uuid::new_v4().as_bytes())),
            root_identity: identity,
        };
        publish(&path, ROOT_MARKER, &serde_json::to_vec(&marker)?)?;
        publish(&path, RUST_ROOT_MARKER, PROTOTYPE)?;
        Self::open(&path, namespaces)
    }

    /// Open only a marked Rust prototype root, rejecting legacy layouts before database access.
    pub fn open(path: &Path, namespaces: &RootNamespaces) -> io::Result<Self> {
        let (canonical_path, marker) = inspect(path)?;
        lock::private_directory(&namespaces.ownership)?;
        let lock_path = namespaces
            .ownership
            .join(format!("{}.lock", marker.root_id));
        let durable_lease = lock::acquire(&lock_path)?;
        lock::private_directory(&namespaces.control)?;
        let control_directory = namespaces.control.join(&marker.root_id);
        lock::private_directory(&control_directory)?;
        let compatibility_lease = lock::acquire(&control_directory.join("owner.lock"))?;
        let owner = Self {
            canonical_path,
            marker,
            control_directory,
            lock_path,
            durable_lease,
            compatibility_lease,
        };
        owner.validate_current()?;
        Ok(owner)
    }

    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }
    pub fn root_id(&self) -> &str {
        &self.marker.root_id
    }
    pub fn kind(&self) -> &str {
        &self.marker.kind
    }
    pub fn control_directory(&self) -> &Path {
        &self.control_directory
    }
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Call before root mutations. Detect path rebinding, marker replacement and lease unlinking.
    pub fn validate_current(&self) -> io::Result<()> {
        self.check_directory()?;
        let marker = read_marker(&self.canonical_path)?;
        self.check_directory()?;
        if marker != self.marker {
            return Err(io::Error::other("root marker identity changed"));
        }
        lock::stable(&self.durable_lease, &self.lock_path)?;
        lock::stable(
            &self.compatibility_lease,
            &self.control_directory.join("owner.lock"),
        )?;
        Ok(())
    }

    fn check_directory(&self) -> io::Result<()> {
        if self.canonical_path.canonicalize()? != self.canonical_path
            || directory_identity(&self.canonical_path)? != self.marker.root_identity
        {
            return Err(io::Error::other("root directory identity changed"));
        }
        Ok(())
    }
}

fn directory_identity(path: &Path) -> io::Result<Identity> {
    let metadata = path.symlink_metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::other("State Root must be a directory"));
    }
    #[cfg(not(windows))]
    let (dev, ino) = lock::identity(&metadata)?;
    #[cfg(windows)]
    let (dev, ino) = {
        let file = windows::open_nofollow(path, false)?;
        let identity = windows::file_identity(&file)?;
        (identity.volume, identity.index)
    };
    Ok(Identity {
        dev: dev.to_string(),
        ino: ino.to_string(),
    })
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let file = lock::open_regular(path, false)?;
    let before = file.metadata()?;
    let mut bytes = Vec::new();
    (&file).take(1025).read_to_end(&mut bytes)?;
    lock::stable(&file, path)?;
    let after = file.metadata()?;
    if bytes.len() > 1024
        || before.len() != bytes.len() as u64
        || before.len() != after.len()
        || before.modified()? != after.modified()?
    {
        return Err(io::Error::other("invalid or changing bounded marker"));
    }
    Ok(bytes)
}

fn read_marker(path: &Path) -> io::Result<Marker> {
    let marker: Marker = serde_json::from_slice(&read_bounded(&path.join(ROOT_MARKER))?)?;
    if marker.schema_version != 1
        || marker.kind != "interactive"
        || marker.root_id.len() != 64
        || !marker
            .root_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(io::Error::other("invalid State Root marker"));
    }
    Ok(marker)
}

fn check_layout(path: &Path) -> io::Result<()> {
    if read_bounded(&path.join(RUST_ROOT_MARKER))? != PROTOTYPE {
        return Err(io::Error::other(
            "not a Rust prototype root; legacy state is unsupported",
        ));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        // Tolerate an old experimental file without reading, following or deleting it.
        if name == "model-facts.json" {
            continue;
        }
        // These dedicated data directories are not alternate database layouts.
        // Do not accept a symlink/reparse point in place of either directory.
        if (name == "skills" || name == "workhub-coordination") && entry.file_type()?.is_dir() {
            continue;
        }
        if !matches!(
            name.to_str(),
            Some(
                ROOT_MARKER
                    | RUST_ROOT_MARKER
                    | ROOT_DATABASE
                    | "runtime-rust.sqlite-wal"
                    | "runtime-rust.sqlite-shm"
                    | "runtime-rust.sqlite.writer.lock"
                    | "configuration-rust.sqlite"
                    | "configuration-rust.sqlite-wal"
                    | "configuration-rust.sqlite-shm"
            )
        ) || !entry.file_type()?.is_file()
        {
            return Err(io::Error::other("unsupported files in Rust prototype root"));
        }
    }
    Ok(())
}

fn publish(root: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let temp = root.join(format!("{name}.{}.tmp", uuid::Uuid::new_v4()));
    #[cfg(not(windows))]
    let mut file = {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&temp)?
    };
    #[cfg(windows)]
    let mut file = windows::create_private_file(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        #[cfg(not(windows))]
        {
            fs::hard_link(&temp, root.join(name))?;
            File::open(root)?.sync_all()
        }
        #[cfg(windows)]
        windows::publish_file(&temp, &root.join(name))
    })();
    let cleanup = match fs::remove_file(&temp) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    };
    result.and(cleanup)
}
