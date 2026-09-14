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

use super::{Code, error};
use maka_protocol::OperationError;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

type Key = (String, String);
type Result<T> = std::result::Result<T, OperationError>;
const TTL: Duration = Duration::from_secs(300);
const MAX_ACTIVE: usize = 16;
const MAX_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Manifest {
    pub name: String,
    pub mime_type: String,
    pub total_bytes: u64,
    pub digest: String,
}

/// Each Upload owns its full declared byte budget until the payload owner drops.
/// Moving it into the database job therefore does not free capacity prematurely.
pub(super) struct Upload {
    pub manifest: Manifest,
    bytes: Vec<u8>,
    owner: Uuid,
    expires: Instant,
    _capacity: OwnedSemaphorePermit,
}

impl AsRef<[u8]> for Upload {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// Host-owned staging implies epoch affinity; connection IDs are never reused.
pub(in crate::server) struct Uploads {
    entries: Mutex<HashMap<Key, Upload>>,
    capacity: Arc<Semaphore>,
}

impl Default for Uploads {
    fn default() -> Self {
        Self {
            entries: Mutex::default(),
            capacity: Arc::new(Semaphore::new(MAX_BYTES)),
        }
    }
}

impl Uploads {
    fn entries(&self) -> Result<MutexGuard<'_, HashMap<Key, Upload>>> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| error(Code::PersistenceFailed, "Artifact staging is unavailable"))?;
        let now = Instant::now();
        entries.retain(|_, upload| upload.expires > now);
        Ok(entries)
    }

    pub(super) fn open(&self, key: Key, owner: Uuid, manifest: Manifest) -> Result<u64> {
        let mut entries = self.entries()?;
        if let Some(upload) = entries.get_mut(&key) {
            if upload.owner != owner || upload.manifest != manifest {
                return Err(error(
                    Code::OperationConflict,
                    "Upload identity is already in use",
                ));
            }
            upload.expires = Instant::now() + TTL;
            return Ok(upload.bytes.len() as u64);
        }
        if entries.len() >= MAX_ACTIVE {
            return Err(error(
                Code::OperationConflict,
                "Attachment upload capacity is exhausted",
            ));
        }
        let capacity = self
            .capacity
            .clone()
            .try_acquire_many_owned(manifest.total_bytes as u32)
            .map_err(|_| {
                error(
                    Code::OperationConflict,
                    "Attachment upload capacity is exhausted",
                )
            })?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(manifest.total_bytes as usize)
            .map_err(|_| {
                error(
                    Code::OperationConflict,
                    "Attachment upload capacity is exhausted",
                )
            })?;
        entries.insert(
            key,
            Upload {
                manifest,
                bytes,
                owner,
                expires: Instant::now() + TTL,
                _capacity: capacity,
            },
        );
        Ok(0)
    }

    pub fn accept(&self, key: &Key, owner: Uuid, offset: u64, bytes: &[u8]) -> Result<u64> {
        let mut entries = self.entries()?;
        let upload = entries
            .get_mut(key)
            .filter(|upload| upload.owner == owner)
            .ok_or_else(|| error(Code::NotFound, "Attachment upload was not found"))?;
        let next = upload.bytes.len() as u64;
        let end = offset
            .checked_add(bytes.len() as u64)
            .filter(|end| *end <= upload.manifest.total_bytes && offset <= next)
            .ok_or_else(|| error(Code::OperationConflict, "Invalid attachment chunk offset"))?;
        if offset < next {
            if end > next || upload.bytes[offset as usize..end as usize] != *bytes {
                return Err(error(
                    Code::OperationConflict,
                    "Attachment chunk replay differs",
                ));
            }
        } else {
            upload.bytes.extend_from_slice(bytes);
        }
        upload.expires = Instant::now() + TTL;
        Ok(upload.bytes.len() as u64)
    }

    pub(super) fn consume(&self, key: &Key, owner: Uuid) -> Result<Upload> {
        let mut entries = self.entries()?;
        let upload = entries
            .get(key)
            .filter(|upload| upload.owner == owner)
            .ok_or_else(|| error(Code::NotFound, "Attachment upload was not found"))?;
        if upload.bytes.len() as u64 != upload.manifest.total_bytes {
            return Err(error(
                Code::OperationConflict,
                "Attachment upload is incomplete",
            ));
        }
        Ok(entries.remove(key).expect("checked owned upload"))
    }

    pub fn abort(&self, key: &Key, owner: Uuid) -> Result<()> {
        let mut entries = self.entries()?;
        if entries.get(key).is_some_and(|upload| upload.owner == owner) {
            entries.remove(key);
        }
        Ok(())
    }

    pub fn connection(&self, owner: Uuid) -> Connection<'_> {
        Connection {
            uploads: self,
            owner,
        }
    }
}

pub(in crate::server) struct Connection<'a> {
    uploads: &'a Uploads,
    owner: Uuid,
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        let mut entries = self
            .uploads
            .entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        entries.retain(|_, upload| upload.owner != self.owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_exact_replay_expiry_and_consumed_buffer_budget_share_one_lifetime() {
        let uploads = Uploads::default();
        let owner = Uuid::new_v4();
        let other = Uuid::new_v4();
        let connection = uploads.connection(owner);
        let key = ("session".into(), "upload".into());
        let manifest = Manifest {
            name: "x".into(),
            mime_type: "x".into(),
            total_bytes: 3,
            digest: "digest".into(),
        };
        uploads.open(key.clone(), owner, manifest.clone()).unwrap();
        assert_eq!(
            uploads
                .open(key.clone(), other, manifest.clone())
                .unwrap_err()
                .code,
            Code::OperationConflict
        );
        assert_eq!(
            uploads.accept(&key, other, 0, b"ab").unwrap_err().code,
            Code::NotFound
        );
        uploads.abort(&key, other).unwrap();
        uploads.accept(&key, owner, 0, b"ab").unwrap();
        assert_eq!(uploads.accept(&key, owner, 0, b"ab").unwrap(), 2);
        assert_eq!(
            uploads.open(key.clone(), owner, manifest.clone()).unwrap(),
            2
        );
        assert!(uploads.accept(&key, owner, 1, b"bc").is_err());
        assert!(uploads.accept(&key, owner, 0, b"xx").is_err());
        assert!(uploads.consume(&key, owner).is_err());
        uploads.accept(&key, owner, 2, b"c").unwrap();
        let consumed = uploads.consume(&key, owner).unwrap();
        assert_eq!(consumed.as_ref(), b"abc");
        drop(connection);
        assert_eq!(uploads.capacity.available_permits(), MAX_BYTES - 3);
        drop(consumed);
        assert_eq!(uploads.capacity.available_permits(), MAX_BYTES);
        uploads.open(key.clone(), owner, manifest.clone()).unwrap();
        uploads
            .entries
            .lock()
            .unwrap()
            .get_mut(&key)
            .unwrap()
            .expires = Instant::now();
        assert_eq!(
            uploads.accept(&key, owner, 0, b"a").unwrap_err().code,
            Code::NotFound
        );
        uploads.open(key, owner, manifest).unwrap();
        drop(uploads.connection(owner));
        assert_eq!(uploads.capacity.available_permits(), MAX_BYTES);
        assert!(uploads.entries().unwrap().is_empty());
    }
}
