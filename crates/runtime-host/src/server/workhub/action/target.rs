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

use super::{Host, OperationError};
use crate::plugins::workhub::target as policy;
use maka_protocol::workhub::ActInput;
pub(super) use policy::Target;

/// Canonical accepted correction recovery does not need a live provider.
pub(super) async fn recreate(
    host: &Host,
    input: &ActInput,
    title: &str,
) -> Result<Target, OperationError> {
    let (request, spec) = policy::creation(input, title)?;
    let id = request.session_id.clone();
    let creation = host
        .executions
        .prepare_session(request)
        .await
        .map_err(policy::context_error)?;
    let target = Target::Created {
        id,
        creation: Box::new(creation),
        spec,
    };
    validate(host, &target).await?;
    Ok(target)
}

pub(super) async fn validate(host: &Host, target: &Target) -> Result<(), OperationError> {
    if let Target::Created { creation, .. } = target {
        host.executions.validate_creation(creation).await?;
        super::super::super::projects::record_usage(host, &creation.configuration.workspace)
            .await?;
    }
    Ok(())
}
