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

use super::{Failure, Manifest, Prepared, Read, Saved, Ticket, Transfer};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use maka_client::Client;
use maka_protocol::{
    artifact::{
        self, ArtifactIngestInput as Input, ArtifactIngestResult as Output, ArtifactQueryInput,
        ArtifactQueryResult,
    },
    turn::{AttachmentKind, AttachmentRef, StorageRef},
};
use std::{
    io::Read as _,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
};

const MAX_ENTRIES: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Browse {
    pub generation: u64,
    pub session: String,
    pub input: Option<String>,
    pub path: PathBuf,
}
pub struct Entry {
    pub path: PathBuf,
    pub directory: bool,
    pub bytes: u64,
}
pub enum Listing {
    Directory {
        path: PathBuf,
        entries: Vec<Entry>,
        truncated: bool,
    },
    File(PathBuf),
}

pub async fn browse(request: &Browse) -> Result<Listing, Failure> {
    let path = request.path.clone();
    tokio::task::spawn_blocking(move || {
        let path = if path.as_os_str().is_empty() {
            std::env::current_dir().map_err(io_error)?
        } else {
            path
        };
        if !valid_path(&path) {
            return Err(Failure::Invalid);
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(io_error)?;
        if metadata.is_file() {
            if metadata.len() > artifact::MAX_ATTACHMENT_BYTES {
                return Err(Failure::TooLarge);
            }
            return Ok(Listing::File(path));
        }
        if !metadata.is_dir() {
            return Err(Failure::Invalid);
        }
        let mut entries = Vec::new();
        let mut truncated = false;
        for (index, entry) in std::fs::read_dir(&path).map_err(io_error)?.enumerate() {
            if index == MAX_ENTRIES {
                truncated = true;
                break;
            }
            let entry = entry.map_err(io_error)?;
            let metadata = entry.metadata().map_err(io_error)?;
            if valid_path(&entry.path()) && (metadata.is_dir() || metadata.is_file()) {
                entries.push(Entry {
                    path: entry.path(),
                    directory: metadata.is_dir(),
                    bytes: metadata.len(),
                });
            }
        }
        entries.sort_by(|a, b| {
            b.directory
                .cmp(&a.directory)
                .then_with(|| a.path.file_name().cmp(&b.path.file_name()))
        });
        Ok(Listing::Directory {
            path,
            entries,
            truncated,
        })
    })
    .await
    .map_err(|e| Failure::Io(e.to_string()))?
}
fn io_error(error: std::io::Error) -> Failure {
    Failure::Io(error.to_string())
}
pub fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|s| s.len() <= 4096 && !s.chars().any(char::is_control))
}
impl Manifest {
    pub fn begin(&self, ticket: &Ticket) -> Input {
        Input::Begin {
            session_id: ticket.session.clone(),
            upload_id: ticket.id.clone(),
            name: self.name.clone(),
            mime_type: self.mime.clone(),
            total_bytes: self.bytes,
            content_sha256: self.digest.clone(),
        }
    }
    fn reference(&self, ticket: &Ticket) -> AttachmentRef {
        AttachmentRef {
            name: self.name.clone(),
            mime_type: self.mime.clone(),
            bytes: self.bytes,
            kind: AttachmentKind::from_metadata(&self.mime, &self.name),
            storage_ref: StorageRef::SessionFile {
                session_id: ticket.session.clone(),
                relative_path: artifact::upload_artifact_id(&ticket.session, &ticket.id),
            },
        }
    }
    fn matches(&self, ticket: &Ticket, reference: &AttachmentRef) -> bool {
        self.reference(ticket) == *reference
    }
}
impl Saved {
    pub fn validate(&self, session: &str) -> Result<(), String> {
        if !valid_path(&self.path) || uuid::Uuid::parse_str(&self.id).is_err() {
            return Err("Invalid attachment draft path or identity".into());
        }
        let ticket = Ticket {
            root: String::new(),
            epoch: String::new(),
            session: session.into(),
            input: None,
            id: self.id.clone(),
            generation: 0,
        };
        if let Some(manifest) = &self.manifest {
            artifact::decode_ingest_input(&serde_json::to_value(manifest.begin(&ticket)).unwrap())
                .map_err(|e| e.to_string())?;
            if manifest.name != artifact::normalize_name(&manifest.name)
                || self
                    .attachment
                    .as_ref()
                    .is_some_and(|a| !manifest.matches(&ticket, a))
            {
                return Err("Invalid attachment draft manifest".into());
            }
        } else if self.attachment.is_some() {
            return Err("Attachment draft has no manifest".into());
        }
        Ok(())
    }
}

pub async fn prepare(
    client: &Client,
    ticket: &Ticket,
    saved: Saved,
    transfer: Arc<Transfer>,
) -> Result<Read, Failure> {
    // Explicit recovery may resolve a committed upload even if the local file is now absent.
    if let Some(manifest) = &saved.manifest {
        let result = client
            .query_artifact(ArtifactQueryInput::Get {
                session_id: ticket.session.clone(),
                artifact_id: artifact::upload_artifact_id(&ticket.session, &ticket.id),
            })
            .await
            .map_err(|e| Failure::Host(e.to_string()))?;
        if let ArtifactQueryResult::Artifact {
            artifact: Some(record),
            ..
        } = result
        {
            if record.name != manifest.name
                || record.mime_type.as_deref() != Some(&manifest.mime)
                || record.size_bytes != manifest.bytes
                || record.summary.as_deref() != Some(&manifest.digest)
                || record.turn_id != ticket.id
            {
                return Err(Failure::Changed);
            }
            return Ok(Read::Recovered(manifest.reference(ticket)));
        }
    }
    tokio::task::spawn_blocking(move || read_file(&saved, &transfer))
        .await
        .map_err(|e| Failure::Io(e.to_string()))?
        .map(Read::Prepared)
}
fn read_file(saved: &Saved, transfer: &Transfer) -> Result<Prepared, Failure> {
    if !valid_path(&saved.path) {
        return Err(Failure::Invalid);
    }
    let before = std::fs::symlink_metadata(&saved.path).map_err(io_error)?;
    if !before.is_file() {
        return Err(Failure::Invalid);
    }
    if before.len() > artifact::MAX_ATTACHMENT_BYTES {
        return Err(Failure::TooLarge);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A raced FIFO must not block; a raced symlink must not redirect this explicit selection.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(&saved.path).map_err(io_error)?;
    let opened = file.metadata().map_err(io_error)?;
    if !opened.is_file() {
        return Err(Failure::Invalid);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if opened.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return Err(Failure::Invalid);
        }
    }
    if opened.len() > artifact::MAX_ATTACHMENT_BYTES {
        return Err(Failure::TooLarge);
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    let mut chunk = [0; 64 * 1024];
    loop {
        if transfer.cancelled.load(Ordering::Relaxed) {
            return Err(Failure::Invalid);
        }
        let count = file.read(&mut chunk).map_err(io_error)?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > artifact::MAX_ATTACHMENT_BYTES as usize {
            return Err(Failure::TooLarge);
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let after = file.metadata().map_err(io_error)?;
    if after.len() != opened.len()
        || bytes.len() as u64 != opened.len()
        || after.modified().ok() != opened.modified().ok()
    {
        return Err(Failure::Changed);
    }
    let name = saved
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(Failure::Invalid)?;
    let mime = artifact::sniff_binary_mime(&bytes).unwrap_or_else(|| {
        match saved
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "json" => "application/json",
            "csv" => "text/csv",
            "md" | "markdown" => "text/markdown",
            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            _ if !bytes.contains(&0) && std::str::from_utf8(&bytes).is_ok() => "text/plain",
            _ => "application/octet-stream",
        }
    });
    let manifest = Manifest {
        name: artifact::normalize_name(name),
        mime: mime.into(),
        bytes: bytes.len() as u64,
        digest: artifact::content_digest(&bytes),
    };
    if saved.manifest.as_ref().is_some_and(|old| *old != manifest) {
        return Err(Failure::Changed);
    }
    Ok(Prepared { manifest, bytes })
}

pub async fn upload(
    client: &Client,
    ticket: &Ticket,
    prepared: Prepared,
    transfer: Arc<Transfer>,
) -> Result<AttachmentRef, Failure> {
    let result = upload_inner(client, ticket, &prepared, &transfer).await;
    // Removal only detaches a draft. Never delete a possibly committed artifact.
    if transfer.cancelled.load(Ordering::Relaxed) {
        let _ = client
            .ingest_artifact(Input::Abort {
                session_id: ticket.session.clone(),
                upload_id: ticket.id.clone(),
            })
            .await;
    }
    result
}
async fn upload_inner(
    client: &Client,
    ticket: &Ticket,
    prepared: &Prepared,
    transfer: &Transfer,
) -> Result<AttachmentRef, Failure> {
    if transfer.cancelled.load(Ordering::Relaxed) {
        return Err(Failure::Invalid);
    }
    let opened = client
        .ingest_artifact(prepared.manifest.begin(ticket))
        .await
        .map_err(|e| Failure::Host(e.to_string()))?;
    let reference = match opened {
        Output::Committed { attachment, .. } => attachment,
        Output::UploadOpened { next_offset, .. } => {
            let mut offset = next_offset as usize;
            transfer.bytes.store(next_offset, Ordering::Relaxed);
            while offset < prepared.bytes.len() {
                if transfer.cancelled.load(Ordering::Relaxed) {
                    return Err(Failure::Invalid);
                }
                let end = (offset + artifact::MAX_INGEST_CHUNK_BYTES).min(prepared.bytes.len());
                let accepted = client
                    .ingest_artifact(Input::Chunk {
                        session_id: ticket.session.clone(),
                        upload_id: ticket.id.clone(),
                        offset: offset as u64,
                        chunk_base64: STANDARD.encode(&prepared.bytes[offset..end]),
                    })
                    .await
                    .map_err(|e| Failure::Host(e.to_string()))?;
                let Output::ChunkAccepted { next_offset, .. } = accepted else {
                    return Err(Failure::Invalid);
                };
                if next_offset as usize > prepared.bytes.len() {
                    return Err(Failure::Invalid);
                }
                offset = next_offset as usize;
                transfer.bytes.store(next_offset, Ordering::Relaxed);
            }
            if transfer.cancelled.load(Ordering::Relaxed) {
                return Err(Failure::Invalid);
            }
            let committed = client
                .ingest_artifact(Input::Commit {
                    session_id: ticket.session.clone(),
                    upload_id: ticket.id.clone(),
                })
                .await
                .map_err(|e| Failure::Host(e.to_string()))?;
            let Output::Committed { attachment, .. } = committed else {
                return Err(Failure::Invalid);
            };
            attachment
        }
        _ => return Err(Failure::Invalid),
    };
    if !prepared.manifest.matches(ticket, &reference) {
        return Err(Failure::Changed);
    }
    Ok(reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_reads_are_bounded_regular_files_and_retries_keep_the_original_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("中文.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();
        let mut saved = Saved {
            id: uuid::Uuid::new_v4().to_string(),
            path,
            manifest: None,
            attachment: None,
        };
        let transfer = Transfer::default();
        let prepared = read_file(&saved, &transfer).unwrap();
        assert_eq!(prepared.manifest.mime, "image/png");
        assert_eq!(prepared.bytes, b"\x89PNG\r\n\x1a\n");
        saved.manifest = Some(prepared.manifest);
        saved.validate("session").unwrap();
        std::fs::write(&saved.path, b"changed").unwrap();
        assert!(matches!(
            read_file(&saved, &transfer),
            Err(Failure::Changed)
        ));
        std::fs::File::create(&saved.path)
            .unwrap()
            .set_len(artifact::MAX_ATTACHMENT_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read_file(&saved, &transfer),
            Err(Failure::TooLarge)
        ));
        std::fs::remove_file(&saved.path).unwrap();
        std::fs::create_dir(&saved.path).unwrap();
        assert!(matches!(
            read_file(&saved, &transfer),
            Err(Failure::Invalid)
        ));
        #[cfg(unix)]
        {
            use std::{ffi::CString, os::unix::ffi::OsStrExt};
            std::fs::remove_dir(&saved.path).unwrap();
            std::os::unix::fs::symlink(directory.path().join("missing"), &saved.path).unwrap();
            assert!(matches!(
                read_file(&saved, &transfer),
                Err(Failure::Invalid)
            ));
            std::fs::remove_file(&saved.path).unwrap();
            let name = CString::new(saved.path.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            assert!(matches!(
                read_file(&saved, &transfer),
                Err(Failure::Invalid)
            ));
        }
    }
}
