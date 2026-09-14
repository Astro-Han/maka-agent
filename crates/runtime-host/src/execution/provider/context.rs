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
use super::unavailable;
use maka_config::model_catalog::{ProviderFacts, resolve_limits};
use maka_protocol::OperationError;
use maka_runtime::{configuration::ConnectionCatalogEntry, context::ModelRequestContext};

pub(super) fn resolve(
    connection: &ConnectionCatalogEntry,
    facts: &ProviderFacts,
    model_id: &str,
) -> Result<ModelRequestContext, OperationError> {
    let capacity = resolve_limits(connection, facts, model_id)
        .map_err(|error| unavailable(error.to_string()))?;
    Ok(ModelRequestContext {
        provider_id: connection.provider_type.clone(),
        context_window: capacity.input_budget(),
        declared_window: connection
            .model_overrides
            .as_ref()
            .and_then(|values| values.get(model_id))
            .and_then(|value| value.compaction_threshold),
    })
}
