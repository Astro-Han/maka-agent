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

use super::Inventory;
use crate::StoreError;
use maka_runtime::{
    artifact::Artifact,
    model::ModelUsage,
    pricing::Quote,
    session::{CopyRequest, CopyState, Lineage},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub(super) const MAGIC: &[u8] = b"MAKA-SESSION\0\x01";
pub(super) const MAX_FRAME: usize = 18 * 1024 * 1024;
pub(super) const MAX_BYTES: u64 = 1024 * 1024 * 1024;
pub(super) const MAX_EVENTS: usize = 100_000;
pub(super) const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_BLOB_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const CHUNK: usize = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Record {
    Header {
        inventory: Inventory,
        source_high_water: u64,
    },
    Session {
        id: String,
        parent: Option<String>,
        created_at: u64,
        updated_at: u64,
        archived: bool,
        configuration: serde_json::Value,
    },
    Copy(Copy),
    Member {
        session: String,
        sequence: u64,
        archive_sequence: Option<u64>,
    },
    RevisionSource {
        session: String,
        sequence: u64,
    },
    HistoryArtifact {
        session: String,
        source_session: String,
        source_artifact: String,
        artifact: String,
    },
    /// Preserve source bytes, including ordering, rather than serialize a decoded event again.
    Event {
        sequence: u64,
        json: String,
    },
    Blob(Blob),
    Accounting {
        event_id: String,
        quote: Quote,
        valuation: Option<Valuation>,
    },
    End {
        digest: String,
        frames: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Copy {
    pub request: CopyRequest,
    pub through: u64,
    pub observed_through: u64,
    pub lineage: Lineage,
    pub state: CopyState,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Valuation {
    pub usage: ModelUsage,
    pub usd: Option<f64>,
}

/// Exactly the declared raw bytes follow this frame; binary data is not JSON/base64.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "resource", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Blob {
    ToolResult {
        event_id: String,
        bytes: u64,
        digest: String,
    },
    Artifact {
        metadata: Artifact,
        digest: String,
    },
    Composition {
        event_id: String,
        bytes: u64,
        digest: String,
    },
}

impl Blob {
    pub fn bytes(&self) -> u64 {
        match self {
            Self::ToolResult { bytes, .. } | Self::Composition { bytes, .. } => *bytes,
            Self::Artifact { metadata, .. } => metadata.size_bytes,
        }
    }
    pub fn digest(&self) -> &str {
        match self {
            Self::ToolResult { digest, .. }
            | Self::Artifact { digest, .. }
            | Self::Composition { digest, .. } => digest,
        }
    }
}

pub(super) struct Writer<W> {
    output: W,
    digest: Sha256,
    bytes: u64,
    frames: u64,
}

impl<W: AsyncWrite + Unpin> Writer<W> {
    pub async fn new(output: W) -> Result<Self, StoreError> {
        let mut writer = Self {
            output,
            digest: Sha256::new(),
            bytes: 0,
            frames: 0,
        };
        writer.bytes(MAGIC).await?;
        Ok(writer)
    }

    pub async fn record(&mut self, record: &Record) -> Result<(), StoreError> {
        if self.frames >= 1_000_000 {
            return Err(StoreError::PrefixTooLarge);
        }
        let bytes = serde_json::to_vec(record)?;
        if bytes.len() > MAX_FRAME {
            return Err(StoreError::PrefixTooLarge);
        }
        self.bytes(&(bytes.len() as u32).to_be_bytes()).await?;
        self.bytes(&bytes).await?;
        self.frames += 1;
        Ok(())
    }

    pub async fn bytes(&mut self, bytes: &[u8]) -> Result<(), StoreError> {
        if bytes.len() as u64 > MAX_BYTES.saturating_sub(self.bytes) {
            return Err(StoreError::PrefixTooLarge);
        }
        // A stalled destination cannot hold a WAL snapshot or Host shutdown forever.
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.output.write_all(bytes),
        )
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "bundle destination stalled")
        })??;
        self.digest.update(bytes);
        self.bytes += bytes.len() as u64;
        Ok(())
    }

    pub async fn finish(mut self) -> Result<(W, u64, String), StoreError> {
        let digest = format!("sha256:{:x}", self.digest.clone().finalize());
        self.record(&Record::End {
            digest: digest.clone(),
            frames: self.frames,
        })
        .await?;
        tokio::time::timeout(std::time::Duration::from_secs(30), self.output.flush())
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "bundle flush stalled")
            })??;
        Ok((self.output, self.bytes, digest))
    }
}
