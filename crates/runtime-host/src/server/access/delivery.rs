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

#[cfg(not(windows))]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

const PREFIX: &str = "runtime-host-access-delivery-";

// Keep the file open until cleanup so an unlinked inode cannot be reused for a
// replacement. No Debug implementation: delivery contents are credentials.
pub(super) struct Delivery {
    id: String,
    path: PathBuf,
    file: File,
}

impl Delivery {
    pub(super) fn create(
        control: &Path,
        credential_id: &str,
        credential: &str,
    ) -> io::Result<Self> {
        validate_control(control)?;
        if credential.is_empty() || credential.encode_utf16().count() > 512 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid delivery credential",
            ));
        }
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Payload<'a> {
            credential_id: &'a str,
            credential: &'a str,
        }
        let mut bytes = serde_json::to_vec(&Payload {
            credential_id,
            credential,
        })
        .map_err(|_| io::Error::other("cannot encode delivery"))?;
        bytes.push(b'\n');
        if bytes.len() > 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "delivery exceeds size limit",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let path = control.join(format!("{PREFIX}{id}.json"));
        #[cfg(not(windows))]
        let file = {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            options.open(&path)?
        };
        #[cfg(windows)]
        let file = maka_event_log::root::windows::create_private_file(&path)?;
        let mut delivery = Self { id, path, file };
        #[cfg(unix)]
        delivery
            .file
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        delivery.file.write_all(&bytes)?;
        Ok(delivery)
    }

    pub(super) fn id(&self) -> &str {
        &self.id
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let (Ok(owned), Ok(current)) = (self.file.metadata(), fs::symlink_metadata(&self.path))
            && current.is_file()
            && owned.dev() == current.dev()
            && owned.ino() == current.ino()
        {
            let _ = fs::remove_file(&self.path);
        }
        #[cfg(windows)]
        {
            use maka_event_log::root::windows::{file_identity, open_nofollow};
            if let Ok(named) = open_nofollow(&self.path, false)
                && named.metadata().is_ok_and(|m| m.is_file())
                && let (Ok(owned), Ok(current)) = (file_identity(&self.file), file_identity(&named))
                && (owned.volume, owned.index) == (current.volume, current.index)
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

fn validate_control(control: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(control)?;
        // SAFETY: geteuid has no preconditions or pointer arguments.
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "delivery directory must be private and owned by the current account",
            ));
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use maka_event_log::root::windows::{open_nofollow, validate_private};
        let directory = open_nofollow(control, false)?;
        if !directory.metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "private control directory required",
            ));
        }
        validate_private(&directory)
    }
}

pub(super) fn purge(control: &Path) -> io::Result<()> {
    validate_control(control)?;
    for entry in fs::read_dir(control)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|name| name.strip_prefix(PREFIX))
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        let Ok(uuid) = uuid::Uuid::parse_str(id) else {
            continue;
        };
        if uuid.get_version_num() != 4
            || uuid.get_variant() != uuid::Variant::RFC4122
            || uuid.to_string() != id
            || !entry.file_type()?.is_file()
        {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_delivery_and_cleanup_preserve_replacements_and_unrelated_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("control");
        #[cfg(unix)]
        {
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        }
        #[cfg(windows)]
        maka_event_log::root::windows::private_directory(&root).unwrap();
        let delivery = Delivery::create(&root, "id", "secret").unwrap();
        let path = delivery.path.clone();
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("{PREFIX}{}.json", delivery.id())
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            b"{\"credentialId\":\"id\",\"credential\":\"secret\"}\n"
        );
        #[cfg(unix)]
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        #[cfg(windows)]
        maka_event_log::root::windows::validate_private(&delivery.file).unwrap();
        drop(delivery);
        assert!(!path.exists());

        let delivery = Delivery::create(&root, "id", "secret").unwrap();
        let path = delivery.path.clone();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"replacement").unwrap();
        drop(delivery);
        assert_eq!(fs::read(&path).unwrap(), b"replacement");

        let malformed = root.join(format!("{PREFIX}------------------------------------.json"));
        fs::write(&malformed, b"keep").unwrap();
        let directory = root.join(format!("{PREFIX}{}.json", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        #[cfg(unix)]
        let symlink = root.join(format!("{PREFIX}{}.json", uuid::Uuid::new_v4()));
        #[cfg(unix)]
        std::os::unix::fs::symlink(&malformed, &symlink).unwrap();
        purge(&root).unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read(malformed).unwrap(), b"keep");
        assert!(directory.is_dir());
        #[cfg(unix)]
        assert!(fs::symlink_metadata(symlink).unwrap().is_symlink());
    }
}
