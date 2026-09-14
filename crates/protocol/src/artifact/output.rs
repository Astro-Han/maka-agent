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

use super::preview;
use maka_runtime::{artifact::Artifact, attachment::AttachmentRef};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ArtifactIngestResult {
    UploadOpened {
        upload_id: String,
        next_offset: u64,
    },
    ChunkAccepted {
        upload_id: String,
        next_offset: u64,
    },
    UploadAborted {
        upload_id: String,
    },
    Committed {
        upload_id: String,
        attachment: AttachmentRef,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ArtifactQueryResult {
    RevisionChanged {
        expected: String,
        actual: String,
    },
    Artifact {
        session_id: String,
        revision: String,
        #[serde(deserialize_with = "nullable")]
        artifact: Option<Artifact>,
    },
    Page {
        session_id: String,
        revision: String,
        artifacts: Vec<Artifact>,
        #[serde(deserialize_with = "nullable")]
        next_cursor: Option<String>,
    },
    Text {
        session_id: String,
        artifact_id: String,
        #[serde(with = "preview")]
        preview: Result<TextPreview, ReadFailure>,
    },
    Binary {
        session_id: String,
        artifact_id: String,
        #[serde(with = "preview")]
        preview: Result<BinaryPreview, BinaryReadFailure>,
    },
    Chunk {
        session_id: String,
        artifact_id: String,
        offset: u64,
        total_bytes: u64,
        chunk_base64: String,
        #[serde(deserialize_with = "nullable")]
        next_offset: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactDeleteResult {
    Deleted {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextPreview {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BinaryPreview {
    pub base64: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadFailure {
    NotFound,
    TooLarge,
    ReadFailed,
    NotAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryReadFailure {
    NotFound,
    TooLarge,
    ReadFailed,
    NotAllowed,
    UnsupportedMime,
}

// Required, but nullable: a missing continuation is not a terminal continuation.
fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}
