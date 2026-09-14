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

use super::{Output, failure, parsed};
use crate::server::Host;
use maka_config::{ConfigError, model_catalog, projection};
use maka_protocol::{OperationError, OperationErrorCode, configuration as wire};
use serde_json::Value;

pub(super) async fn query(host: &Host, value: &Value) -> Result<Output, OperationError> {
    let input = parsed(wire::decode_catalog_query_input(value)).map_err(failure)?;
    let snapshot = host.configuration.catalog().await.map_err(|error| {
        if matches!(error, ConfigError::CommitUnknown) {
            // This transaction is read-only; failure cannot leave an unknown effect.
            OperationError {
                code: OperationErrorCode::PersistenceFailed,
                message: "Cannot finish reading the connection catalog".into(),
            }
        } else {
            failure(error)
        }
    })?;
    projection::project(&snapshot, &input, model_catalog::resolve)
        .map(Output::Catalog)
        .map_err(failure)
}
