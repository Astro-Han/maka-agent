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
use maka_plugins::composition::Scope;

impl Platform {
    pub(crate) async fn publish_catalog_changes(
        self,
        changes: tokio::sync::broadcast::Sender<serde_json::Value>,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        let mut catalog = self.catalog.subscribe();
        let mut platform = self.subscribe();
        let mut previous = None;
        let mut provider_revision = None;
        loop {
            let revision = *catalog.borrow_and_update();
            if provider_revision != Some(revision) {
                let _ = changes.send(serde_json::json!({"kind":"model.provider.catalog.changed","revision":revision}));
                provider_revision = Some(revision);
            }
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
        self.clients.clear();
    }
}
