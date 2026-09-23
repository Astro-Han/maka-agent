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

use super::effects::Effects;
use crate::server::pricing::Catalog;
use futures_util::future::BoxFuture;
use maka_plugins::{
    authorization::{Boundary, Capability},
    call::Scope,
    execution::CommandError,
    pricing::{Page, Query, Update, Updated},
};
use maka_runtime::tools::ToolError;
use std::sync::Arc;

pub(super) struct Prices {
    catalog: Arc<Catalog>,
    effects: Arc<Effects>,
}
impl Prices {
    pub(super) fn new(catalog: Arc<Catalog>, effects: Arc<Effects>) -> Self {
        Self { catalog, effects }
    }
}
impl maka_plugins::pricing::Prices for Prices {
    fn query(&self, input: Query) -> BoxFuture<'_, Result<Page, CommandError>> {
        Box::pin(async move {
            let _lease = self
                .effects
                .owner
                .resource_call()
                .map_err(|_| CommandError::Revoked)?;
            let host = self.effects.host.upgrade().ok_or(CommandError::Draining)?;
            self.catalog
                .query(input)
                .await
                .map_err(|error| match error {
                    maka_config::ConfigError::Invalid(message) => CommandError::Invalid(message),
                    maka_config::ConfigError::CommitUnknown => {
                        host.begin_drain();
                        CommandError::OutcomeUnknown(
                            "pricing read could not confirm the transaction".into(),
                        )
                    }
                    other => CommandError::Unavailable(other.to_string()),
                })
        })
    }

    fn update(&self, call: Scope, input: Update) -> BoxFuture<'_, Result<Updated, ToolError>> {
        let catalog = self.catalog.clone();
        Box::pin(
            self.effects
                .owned(call, move |host, _owner, call, _cancellation| {
                    Box::pin(async move {
                        // Consent revocation uses this gate too. Once admitted, an edit
                        // and its notification finish even when the awaiting plugin retires.
                        let _gate = host.lock_admission().await;
                        if call.identity.agent().is_some()
                            || !matches!(
                                host.plugin_resource_boundary(&call, Capability::ManagePricing)
                                    .await?,
                                Boundary::Profile
                            )
                        {
                            return Err(CommandError::Denied.into());
                        }
                        match catalog.update(input).await {
                            Ok(result) => Ok(result),
                            Err(maka_config::ConfigError::CommitUnknown) => {
                                host.begin_drain();
                                Err(ToolError::Persistence(
                                    "pricing commit outcome is unknown".into(),
                                ))
                            }
                            Err(maka_config::ConfigError::Invalid(message)) => {
                                Err(ToolError::Failed(message))
                            }
                            Err(error) => Err(ToolError::Io {
                                kind: std::io::ErrorKind::Other,
                                message: error.to_string(),
                            }),
                        }
                    })
                }),
        )
    }
}
