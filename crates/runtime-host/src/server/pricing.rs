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
use maka_protocol::{Operation, Outcome, pricing::Input};
use maka_runtime::pricing::Updated;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

pub(super) async fn execute(
    host: &Host,
    operation: Operation,
    value: &Value,
) -> Result<Outcome, HostError> {
    let output = match maka_protocol::pricing::decode_input(operation, value)? {
        Input::Query(input) => host
            .configuration
            .query_pricing(input)
            .await
            .map(serde_json::to_value),
        Input::Update(input) => host
            .configuration
            .update_pricing(input)
            .await
            .map(|result| {
                if matches!(result, Updated::Committed { .. }) {
                    let revision = host.change_revision.fetch_add(1, Ordering::SeqCst) + 1;
                    let _ = host
                        .changes
                        .send(json!({ "kind": "configuration.changed", "revision": revision }));
                }
                serde_json::to_value(result)
            }),
    };
    Ok(match output {
        Ok(value) => Outcome::success(maka_protocol::pricing::decode_output(operation, &value?)?),
        Err(error) => Outcome::failure(configuration::failure(error)),
    })
}
