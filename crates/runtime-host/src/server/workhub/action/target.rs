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

use super::{Code, Host, OperationError, failure, sessions};
use crate::{plugins::workhub::target as policy, session::PreparedSession};
use maka_protocol::{
    session::SessionCreateTarget,
    workhub::{ActInput, RoutingProposal},
};
pub(super) use policy::Target;

pub(super) async fn prepare(
    host: &std::sync::Arc<Host>,
    input: &ActInput,
    selected: Option<&super::super::selection::SelectedTarget>,
) -> Result<Target, OperationError> {
    match policy::route(input)? {
        RoutingProposal::DelegateExisting { candidate_ref } => {
            if let Some(selected) = selected {
                let now = crate::server::configuration::now()
                    .map_err(|error| failure(Code::InternalFailure, error.to_string()))?;
                let record = super::super::candidates::target(host, &selected.session_id).await?;
                return policy::chosen(selected, candidate_ref, record, now);
            }
            let candidates = super::super::candidates::query(host).await?;
            let target = policy::offered(input, candidate_ref, candidates)?;
            if super::super::candidates::target(host, target.id())
                .await?
                .is_none()
            {
                return Err(failure(
                    Code::OperationConflict,
                    "WorkHub target cannot accept a delegation in its current state",
                ));
            }
            Ok(target)
        }
        RoutingProposal::CreateNew { title } => {
            let (request, spec) = policy::creation(input, title)?;
            let id = request.session_id.clone();
            if let SessionCreateTarget::Executor { executor_id } = &request.target {
                host.executions.executor_binding(&id, executor_id)?;
            }
            let prepared = PreparedSession::new(request)
                .map_err(|error| failure(Code::OperationConflict, error.to_string()))?;
            let workspace = sessions::workspace::resolve(host, prepared.workspace())
                .await
                .map_err(context_error)?;
            let configuration =
                sessions::create::resolve(&host.configuration, prepared, None, workspace)
                    .await
                    .map_err(context_error)?;
            super::super::super::projects::record_usage(host, &configuration.workspace).await?;
            Ok(Target::Created {
                id,
                configuration: Box::new(configuration),
                spec,
            })
        }
    }
}

fn context_error(mut error: OperationError) -> OperationError {
    // Valid action syntax can still refer to a stale model or workspace.
    if error.code == Code::InvalidRequest {
        error.code = Code::OperationConflict;
    }
    error
}
