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

use crate::{
    Error, Transcript,
    source::{Selection, Source},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use maka_plugins::{
    execution::{CreateRoot, RootSettings},
    session::import::{ImportState, Receipt},
    storage::{Data, Mutation, Record, Store, StoreError},
};
use maka_runtime::{
    execution::WorkspaceTarget,
    import::{MAX_IMPORT_BYTES, MAX_IMPORT_RECORDS},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

const PART_BYTES: usize = 512 * 1024;
const MAX_PAYLOAD: usize = MAX_IMPORT_BYTES as usize + 64 * 1024;
const MAX_PARTS: usize = MAX_PAYLOAD.div_ceil(PART_BYTES);

mod delivery;
mod index;
pub use index::{Copy, Page};

/// Explicit destination choices, independent of the source's observed cwd/model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub operation_id: Uuid,
    pub selection: Selection,
    pub workspace: WorkspaceTarget,
    pub settings: RootSettings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Intent {
    pub request: Request,
    pub source: Source,
    pub title: String,
    pub digest: String,
    pub bytes: usize,
    pub parts: usize,
    pub records: usize,
    pub receipt: Option<Receipt>,
}
pub struct Saved {
    revision: u64,
    intent: Intent,
}
impl Saved {
    pub fn intent(&self) -> &Intent {
        &self.intent
    }
}

#[derive(Clone)]
pub struct Repository {
    store: Arc<dyn Store>,
}
impl Repository {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    pub async fn get(&self, operation_id: Uuid) -> Result<Option<Saved>, Error> {
        self.store
            .read(key(operation_id))
            .await?
            .map(decode)
            .transpose()
    }

    /// Metadata and normalized bytes commit together. A lost reply can only
    /// reveal this complete intent or absence, never partially prepared data.
    pub async fn prepare(
        &self,
        request: Request,
        source: Source,
        transcript: Transcript,
    ) -> Result<Saved, Error> {
        if let Some(saved) = self.get(request.operation_id).await? {
            return same(saved, &request);
        }
        source.validate()?;
        CreateRoot {
            managed: false,
            operation_id: request.operation_id.to_string(),
            name: transcript.title.clone(),
            settings: request.settings.clone(),
        }
        .validate()
        .map_err(|_| Error::Invalid("invalid destination settings"))?;
        maka_plugins::authorization::Request {
            operation_id: request.operation_id,
            title: "Import conversation".into(),
            target: maka_plugins::authorization::Target::Workspace {
                workspace: request.workspace.clone(),
                sandbox_mode: request.settings.sandbox_mode,
            },
            capabilities: [maka_plugins::authorization::Capability::Executions].into(),
        }
        .validate()
        .map_err(|_| Error::Invalid("invalid import destination"))?;
        let adapter = match source.location {
            crate::source::Location::Codex { .. } => "codex",
            crate::source::Location::ClaudeCode { .. } => "claude-code",
            crate::source::Location::OpenCode { .. } => "opencode",
        };
        if source.id != request.selection.source_id
            || request.selection.source_revision == 0
            || transcript.source.adapter != adapter
            || transcript.source.session_id != request.selection.session_id
        {
            return Err(Error::Invalid(
                "transcript does not belong to the selected source",
            ));
        }
        transcript.source.validate().map_err(Error::Invalid)?;
        if transcript.records.is_empty() || transcript.records.len() as u64 > MAX_IMPORT_RECORDS {
            return Err(Error::Invalid("invalid imported history size"));
        }
        let mut record_bytes = 0;
        for record in &transcript.records {
            record.validate().map_err(Error::Invalid)?;
            record_bytes += crate::transcript::retained_size(record)? as usize;
            if record_bytes > MAX_IMPORT_BYTES as usize {
                return Err(Error::Invalid("imported history exceeds its budget"));
            }
        }
        if !transcript
            .records
            .iter()
            .any(maka_runtime::import::Record::is_conversation)
        {
            return Err(Error::Invalid("imported history has no conversation"));
        }
        let bytes = serde_json::to_vec(&transcript)
            .map_err(|_| Error::Invalid("invalid normalized transcript"))?;
        if bytes.len() > MAX_PAYLOAD {
            return Err(Error::Invalid("normalized transcript exceeds its budget"));
        }
        let intent = Intent {
            request: request.clone(),
            source,
            title: transcript.title,
            digest: digest(&bytes),
            bytes: bytes.len(),
            parts: bytes.len().div_ceil(PART_BYTES),
            records: transcript.records.len(),
            receipt: None,
        };
        let mut mutations = vec![Mutation {
            key: key(request.operation_id),
            expected_revision: None,
            data: Data::Present(serde_json::to_value(&intent).map_err(encoding)?),
        }];
        for (index, part) in bytes.chunks(PART_BYTES).enumerate() {
            mutations.push(Mutation {
                key: part_key(request.operation_id, index),
                expected_revision: None,
                data: Data::Present(STANDARD.encode(part).into()),
            });
        }
        match self.store.batch(mutations).await {
            Ok(records) => decode(
                records
                    .into_iter()
                    .next()
                    .ok_or_else(|| encoding("missing intent receipt"))?,
            ),
            Err(StoreError::Conflict { .. }) => same(
                self.get(request.operation_id)
                    .await?
                    .ok_or(Error::Conflict)?,
                &request,
            ),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn transcript(&self, saved: &Saved) -> Result<Transcript, Error> {
        let intent = &saved.intent;
        if intent.receipt.is_some() {
            return Err(Error::Invalid(
                "completed import no longer needs its payload",
            ));
        }
        let mut bytes = Vec::with_capacity(intent.bytes);
        for index in 0..intent.parts {
            let part = self
                .store
                .read(part_key(intent.request.operation_id, index))
                .await?
                .ok_or_else(|| encoding("missing normalized import part"))?;
            let Some(value) = part.data.value().and_then(serde_json::Value::as_str) else {
                return Err(Error::Invalid("invalid normalized import part"));
            };
            if value.len() > PART_BYTES.div_ceil(3) * 4 {
                return Err(Error::Invalid("import part exceeds its budget"));
            }
            let part = STANDARD
                .decode(value)
                .map_err(|_| Error::Invalid("invalid import encoding"))?;
            let expected = (intent.bytes - index * PART_BYTES).min(PART_BYTES);
            if part.len() != expected {
                return Err(Error::Invalid("incomplete normalized import"));
            }
            bytes.extend_from_slice(&part);
        }
        if digest(&bytes) != intent.digest {
            return Err(Error::Invalid("normalized import digest differs"));
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::Invalid("invalid normalized import"))
    }

    /// Canonical completion remains Host-owned. Save its receipt and release all
    /// preparation bytes in the same transaction; incomplete attempts remain recoverable.
    pub async fn complete(&self, saved: Saved, receipt: Receipt) -> Result<Saved, Error> {
        if receipt.progress.state == ImportState::Collecting {
            return Err(Error::Invalid("import has no terminal receipt"));
        }
        if receipt.progress.state == ImportState::Published
            && receipt.progress.records != saved.intent.records as u64
        {
            return Err(Error::Invalid(
                "published receipt does not cover the prepared history",
            ));
        }
        if let Some(existing) = &saved.intent.receipt {
            return if same_receipt(existing, &receipt) {
                Ok(saved)
            } else {
                Err(Error::Conflict)
            };
        }
        let id = saved.intent.request.operation_id;
        let mut intent = saved.intent;
        intent.receipt = Some(receipt.clone());
        let mut mutations = vec![Mutation {
            key: key(id),
            expected_revision: Some(saved.revision),
            data: Data::Present(serde_json::to_value(&intent).map_err(encoding)?),
        }];
        for index in 0..intent.parts {
            mutations.push(Mutation {
                key: part_key(id, index),
                expected_revision: Some(1),
                data: Data::Deleted,
            });
        }
        match self.store.batch(mutations).await {
            Ok(records) => decode(
                records
                    .into_iter()
                    .next()
                    .ok_or_else(|| encoding("missing completion receipt"))?,
            ),
            Err(StoreError::Conflict { .. }) => {
                let current = self.get(id).await?.ok_or(Error::Conflict)?;
                if current
                    .intent
                    .receipt
                    .as_ref()
                    .is_some_and(|current| same_receipt(current, &receipt))
                {
                    Ok(current)
                } else {
                    Err(Error::Conflict)
                }
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn decode(record: Record) -> Result<Saved, Error> {
    let Data::Present(value) = record.data else {
        return Err(Error::Conflict);
    };
    let intent: Intent = serde_json::from_value(value).map_err(encoding)?;
    if intent.bytes == 0
        || intent.bytes > MAX_PAYLOAD
        || intent.parts == 0
        || intent.parts > MAX_PARTS
        || intent.records == 0
        || intent.records as u64 > MAX_IMPORT_RECORDS
        || intent.parts != intent.bytes.div_ceil(PART_BYTES)
        || intent.digest.len() != 64
        || !intent.digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        || intent
            .receipt
            .as_ref()
            .is_some_and(|receipt| receipt.progress.state == ImportState::Collecting)
    {
        return Err(Error::Invalid("invalid persisted import intent"));
    }
    Ok(Saved {
        revision: record.revision,
        intent,
    })
}
fn same(saved: Saved, request: &Request) -> Result<Saved, Error> {
    if &saved.intent.request != request {
        Err(Error::Conflict)
    } else {
        Ok(saved)
    }
}
fn same_receipt(a: &Receipt, b: &Receipt) -> bool {
    a.session_id == b.session_id && a.progress == b.progress
}
fn key(id: Uuid) -> String {
    format!("intents/{id}")
}
fn part_key(id: Uuid, index: usize) -> String {
    format!("payloads/{id}/{index:04}")
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn encoding(error: impl std::fmt::Display) -> StoreError {
    StoreError::Unavailable(error.to_string())
}
