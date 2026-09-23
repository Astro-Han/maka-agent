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

use maka_agent::{RunError, pricing::Pricing};
use maka_config::{ConfigError, ConfigurationStore};
use maka_event_log::connection::BoxFuture;
use maka_runtime::{event::CommitError, pricing::Quote};
use std::sync::Arc;

pub(super) struct Prices(pub Arc<ConfigurationStore>);
impl Pricing for Prices {
    fn quote<'a>(
        &'a self,
        provider: &'a str,
        model: &'a str,
    ) -> BoxFuture<'a, Result<Quote, RunError>> {
        Box::pin(async move {
            self.0
                .quote_model(provider.into(), model.into())
                .await
                .map_err(|error| match error {
                    ConfigError::CommitUnknown => {
                        CommitError::OutcomeUnknown(error.to_string()).into()
                    }
                    _ => RunError::Internal(format!("model pricing unavailable: {error}")),
                })
        })
    }
}
