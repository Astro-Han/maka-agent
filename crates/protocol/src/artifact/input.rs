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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ArtifactIngestInput {
    Begin {
        session_id: String,
        upload_id: String,
        name: String,
        mime_type: String,
        total_bytes: u64,
        content_sha256: String,
    },
    Chunk {
        session_id: String,
        upload_id: String,
        offset: u64,
        chunk_base64: String,
    },
    Commit {
        session_id: String,
        upload_id: String,
    },
    Abort {
        session_id: String,
        upload_id: String,
    },
}
impl ArtifactIngestInput {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Begin { session_id, .. }
            | Self::Chunk { session_id, .. }
            | Self::Commit { session_id, .. }
            | Self::Abort { session_id, .. } => session_id,
        }
    }
    pub fn upload_id(&self) -> &str {
        match self {
            Self::Begin { upload_id, .. }
            | Self::Chunk { upload_id, .. }
            | Self::Commit { upload_id, .. }
            | Self::Abort { upload_id, .. } => upload_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ArtifactQueryInput {
    ListStart {
        session_id: String,
    },
    ListContinue {
        session_id: String,
        revision: String,
        cursor: String,
    },
    Get {
        session_id: String,
        artifact_id: String,
    },
    ReadText {
        session_id: String,
        artifact_id: String,
    },
    ReadBinary {
        session_id: String,
        artifact_id: String,
    },
    ReadChunk {
        session_id: String,
        artifact_id: String,
        offset: u64,
    },
}
impl ArtifactQueryInput {
    pub fn session_id(&self) -> &str {
        match self {
            Self::ListStart { session_id }
            | Self::ListContinue { session_id, .. }
            | Self::Get { session_id, .. }
            | Self::ReadText { session_id, .. }
            | Self::ReadBinary { session_id, .. }
            | Self::ReadChunk { session_id, .. } => session_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactDeleteInput {
    pub session_id: String,
    pub artifact_id: String,
}
