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

use super::{Result, failure, internal};
use maka_plugins::{
    composition::Scope,
    contributions::Catalog,
    input::{Prepared, Request},
};
mod prepared;
pub(crate) use prepared::PreparedMessageInput;

pub(crate) enum Outcome {
    Ready {
        required_tools: std::collections::BTreeSet<String>,
    },
    Blocked {
        message: String,
    },
}

pub(super) async fn prepare(
    catalog: &Catalog,
    request: Request,
    workspace: &maka_plugins::filesystem::ReadRoot,
) -> Result<(Prepared, Outcome)> {
    let scope = Scope::Session(request.session_id.clone());
    let prepared = maka_plugins::input::prepare(catalog, &scope, request, workspace)
        .await
        .map_err(|error| match error {
            maka_plugins::Error::Retired | maka_plugins::Error::Invalid(_) => failure(
                maka_protocol::OperationErrorCode::OperationUnavailable,
                &error.to_string(),
            ),
            other => internal(other),
        })?;
    let selection = match &prepared.blocked {
        Some(message) => Outcome::Blocked {
            message: message.clone(),
        },
        None => Outcome::Ready {
            required_tools: prepared.required_tools.clone(),
        },
    };
    Ok((prepared, selection))
}
