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

use super::super::{
    candidates::eligible,
    control::{Commands, Result, failure},
    selection::workspace_digest,
    target::{self, Target},
};
use maka_protocol::{
    OperationErrorCode as Code,
    workhub::{ActInput, CreateContext, Proposal, RoutingProposal},
};
use maka_runtime::workhub::CorrectionRequest;

/// Accepted intent owns the target choice. Recovery neither discovers a new
/// target nor requires a currently published plugin instance.
pub(crate) async fn prepare(
    commands: &dyn Commands,
    request: &CorrectionRequest,
) -> Result<Target> {
    if let Some(spec) = request.target.create() {
        let input = ActInput {
            turn_id: request.source.turn_id.clone(),
            action_id: request.action_id.clone(),
            proposal: Proposal::Route(RoutingProposal::CreateNew {
                title: spec.title.clone(),
            }),
            candidate_set_id: None,
            create: Some(CreateContext {
                workspace: spec.workspace.clone(),
            }),
            new_work_defaults: spec.defaults.clone(),
            delegation_text: None,
        };
        let (request, spec) = target::creation(&input, &spec.title)?;
        let id = request.session_id.clone();
        let creation = commands
            .prepare_session(request)
            .await
            .map_err(target::context_error)?;
        return Ok(Target::Created {
            id,
            creation: Box::new(creation),
            spec,
        });
    }
    let record = commands
        .target(request.target.session_id().into(), eligible)
        .await?
        .ok_or_else(|| {
            failure(
                Code::OperationConflict,
                "WorkHub replacement target is unavailable",
            )
        })?;
    let workspace_digest = workspace_digest(&record.configuration.workspace);
    if Some(workspace_digest.as_str()) != request.target.workspace_digest() {
        return Err(failure(
            Code::OperationConflict,
            "WorkHub replacement target workspace changed",
        ));
    }
    Ok(Target::Existing {
        id: record.id,
        name: record.configuration.name,
        revision: record.revision,
        configuration_digest: record.configuration_digest,
        workspace_digest,
    })
}
