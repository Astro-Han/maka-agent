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

use super::{Host, HostError, configuration};
use maka_config::ConfigurationStore;
use maka_protocol::{Operation, Outcome, pricing::Input};
use maka_runtime::pricing::{Page, Query, Update, Updated};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// One mutation path for native clients and plugins, including commit notices.
pub(crate) struct Catalog {
    configuration: Arc<ConfigurationStore>,
    changes: tokio::sync::broadcast::Sender<Value>,
    revision: Arc<AtomicU64>,
}
impl Catalog {
    pub(crate) fn new(
        configuration: Arc<ConfigurationStore>,
        changes: tokio::sync::broadcast::Sender<Value>,
        revision: Arc<AtomicU64>,
    ) -> Arc<Self> {
        Arc::new(Self {
            configuration,
            changes,
            revision,
        })
    }
    pub(crate) async fn query(&self, input: Query) -> maka_config::Result<Page> {
        self.configuration.query_pricing(input).await
    }
    pub(crate) async fn update(&self, input: Update) -> maka_config::Result<Updated> {
        let result = self.configuration.update_pricing(input).await?;
        if matches!(result, Updated::Committed { .. }) {
            let revision = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = self
                .changes
                .send(json!({ "kind": "configuration.changed", "revision": revision }));
        }
        Ok(result)
    }
}

pub(super) async fn execute(
    host: &Host,
    operation: Operation,
    value: &Value,
) -> Result<Outcome, HostError> {
    let output = match maka_protocol::pricing::decode_input(operation, value)? {
        Input::Query(input) => host.pricing.query(input).await.map(serde_json::to_value),
        Input::Update(input) => host.pricing.update(input).await.map(serde_json::to_value),
    };
    Ok(match output {
        Ok(value) => Outcome::success(maka_protocol::pricing::decode_output(operation, &value?)?),
        Err(error) => Outcome::failure(configuration::failure(error)),
    })
}
