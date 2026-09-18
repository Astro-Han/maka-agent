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

use super::Platform;
use maka_plugins::{client::Client, composition::Scope, contributions::Contribution};
use maka_protocol::{
    OperationError, OperationErrorCode as Code,
    plugin::{ClientCursor, ClientDescriptor, ClientQuery, ClientResult},
};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(super) struct Cache(Mutex<Option<(u64, Arc<View>)>>);
struct View {
    revision: String,
    entries: Vec<(ClientDescriptor, Contribution<Client>)>,
}
impl Cache {
    fn capture(&self, platform: &Platform) -> Result<Arc<View>, OperationError> {
        let current = platform.catalog.snapshot::<Client>(&Scope::DesktopUi);
        let mut cache = self.0.lock().unwrap();
        if let Some((revision, view)) = &*cache
            && *revision == current.revision
            && view.entries.iter().all(|(_, entry)| entry.is_effective())
        {
            return Ok(view.clone());
        }
        let mut entries = Vec::with_capacity(current.entries.len());
        let mut hash = Sha256::new();
        for (entry_id, entry) in current.entries {
            let identity = entry.owner.identity().map_err(internal)?;
            let bundle = &entry.value.bundle;
            let descriptor = ClientDescriptor {
                entry_id,
                extension_id: identity.package_id,
                activation: identity.activation,
                content_digest: bundle.content_digest.clone(),
                client_digest: bundle.client_digest.clone(),
                sdk_version: bundle.sdk_version,
                total_bytes: bundle.source().len(),
                dependencies: bundle.dependencies.clone(),
                config: entry.value.config.clone(),
            };
            // Bound temporary allocation to one descriptor, not the entire catalog.
            let bytes = serde_json::to_vec(&descriptor).map_err(internal)?;
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
            entries.push((descriptor, entry));
        }
        let view = Arc::new(View {
            revision: format!("sha256-{:x}", hash.finalize()),
            entries,
        });
        *cache = Some((current.revision, view.clone()));
        Ok(view)
    }
}
impl Platform {
    pub(crate) async fn publish_client_changes(
        self,
        changes: tokio::sync::broadcast::Sender<serde_json::Value>,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        let mut catalog = self.catalog.subscribe();
        let mut platform = self.subscribe();
        let mut previous = None;
        loop {
            if let Ok(view) = self.clients.capture(&self) {
                let errors: Vec<_> = self
                    .snapshot()
                    .runtime
                    .entries
                    .iter()
                    .filter(|entry| entry.scope == Scope::DesktopUi)
                    .filter_map(|entry| {
                        entry
                            .error
                            .as_ref()
                            .map(|error| (entry.entry_id.clone(), error.clone()))
                    })
                    .collect();
                let current = (view.revision.clone(), errors);
                if previous
                    .as_ref()
                    .is_some_and(|previous| previous != &current)
                {
                    let _ = changes.send(serde_json::json!({"kind":"plugin.client.changed","revision":view.revision}));
                }
                previous = Some(current);
            }
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                result = catalog.changed() => { if result.is_err() { break; } },
                result = platform.changed() => { if result.is_err() { break; } },
            }
            // Coalesce candidate publication/retirement, not every registration.
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {},
            }
        }
        self.clients.0.lock().unwrap().take();
    }

    /// Only published capabilities contribute to this revision. Diagnostics and
    /// durable intent remain separate platform queries.
    pub fn client_query(&self, query: ClientQuery) -> Result<ClientResult, OperationError> {
        let view = self.clients.capture(self)?;
        match query {
            ClientQuery::Snapshot { cursor } => {
                if cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.revision != view.revision)
                {
                    return Err(failure(Code::StaleCursor, "Client composition changed"));
                }
                let start = match cursor {
                    Some(cursor) => view
                        .entries
                        .iter()
                        .position(|(entry, _)| entry.entry_id == cursor.after_entry)
                        .map(|index| index + 1)
                        .ok_or_else(|| failure(Code::InvalidRequest, "Unknown client cursor"))?,
                    None => 0,
                };
                let mut entries = Vec::new();
                let mut bytes = 0;
                for (entry, _) in view.entries.iter().skip(start).take(32) {
                    let size = serde_json::to_vec(entry).map_err(internal)?.len();
                    if !entries.is_empty() && bytes + size > 96 * 1024 {
                        break;
                    }
                    bytes += size;
                    entries.push(entry.clone());
                }
                let next_cursor =
                    (start + entries.len() < view.entries.len()).then(|| ClientCursor {
                        revision: view.revision.clone(),
                        after_entry: entries
                            .last()
                            .expect("nonempty client page")
                            .entry_id
                            .clone(),
                    });
                Ok(ClientResult::Snapshot {
                    revision: view.revision.clone(),
                    entries,
                    next_cursor,
                })
            }
            ClientQuery::Bundle {
                entry_id,
                activation,
                client_digest,
                offset,
            } => {
                let (_, entry) = view
                    .entries
                    .iter()
                    .find(|(entry, _)| {
                        entry.entry_id == entry_id
                            && entry.activation == activation
                            && entry.client_digest == client_digest
                    })
                    .ok_or_else(|| {
                        failure(
                            Code::OperationConflict,
                            "Client activation or bundle retired",
                        )
                    })?;
                let _lease = entry
                    .admit()
                    .map_err(|_| failure(Code::OperationConflict, "Client activation retired"))?;
                let source = entry.value.bundle.source();
                if !source.is_char_boundary(offset) {
                    return Err(failure(
                        Code::InvalidRequest,
                        "Client offset is not a UTF-8 boundary",
                    ));
                }
                let mut end = source.len().min(offset.saturating_add(16 * 1024));
                while !source.is_char_boundary(end) {
                    end -= 1;
                }
                Ok(ClientResult::Bundle {
                    entry_id,
                    activation,
                    client_digest,
                    offset,
                    total_bytes: source.len(),
                    content: source[offset..end].into(),
                    next_offset: (end < source.len()).then_some(end),
                })
            }
        }
    }
}
fn internal(error: impl ToString) -> OperationError {
    failure(Code::InternalFailure, error.to_string())
}
fn failure(code: Code, message: impl Into<String>) -> OperationError {
    OperationError {
        code,
        message: message.into(),
    }
}
