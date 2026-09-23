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

use super::{Repository, Saved, decode, key};
use crate::Error;
use maka_plugins::{session::import::Receipt, storage::Scan};
use serde::Serialize;
use uuid::Uuid;

/// A bounded management projection; source paths, settings and normalized text
/// remain in the intent and are not repeated for every list entry.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Copy {
    pub operation_id: Uuid,
    pub source_id: Uuid,
    pub source_name: String,
    pub source_session_id: String,
    pub title: String,
    pub records: usize,
    pub receipt: Option<Receipt>,
}
impl Saved {
    pub fn copy(&self) -> Copy {
        Copy {
            operation_id: self.intent.request.operation_id,
            source_id: self.intent.source.id,
            source_name: self.intent.source.name.clone(),
            source_session_id: self.intent.request.selection.session_id.clone(),
            title: self.intent.title.clone(),
            records: self.intent.records,
            receipt: self.intent.receipt.clone(),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub copies: Vec<Copy>,
    pub next: Option<Uuid>,
}
impl Repository {
    pub async fn list(&self, after: Option<Uuid>) -> Result<Page, Error> {
        let page = self
            .store
            .scan(Scan {
                prefix: "intents/".into(),
                after: after.map(key),
            })
            .await?;
        let mut result = Page {
            copies: Vec::new(),
            next: None,
        };
        let mut has_more = page.entries.len() > 16 || page.next_after.is_some();
        let mut bytes = 128; // Page and continuation envelope.
        for entry in page.entries.into_iter().take(16) {
            let saved = decode(entry.record)?;
            if entry.key != key(saved.intent.request.operation_id) {
                return Err(Error::Invalid("import index identity differs"));
            }
            let copy = saved.copy();
            let size = serde_json::to_vec(&copy)
                .map_err(|_| Error::Invalid("invalid import index"))?
                .len()
                + 1;
            if bytes + size > 48 * 1024 {
                if result.copies.is_empty() {
                    return Err(Error::Invalid("import index entry exceeds its budget"));
                }
                has_more = true;
                break;
            }
            bytes += size;
            result.copies.push(copy);
        }
        if has_more {
            result.next = result.copies.last().map(|copy| copy.operation_id);
        }
        Ok(result)
    }
}
