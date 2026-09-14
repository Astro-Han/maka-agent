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

use super::{Executions, Result, Selection, failure, internal, prepare};
use maka_protocol::{
    OperationErrorCode as Code,
    turn::{TurnResumeQueryInput, TurnResumeStartResult},
};
use maka_runtime::{
    event::Invocation,
    workhub::{ResumeOrigin, resumed_turn_id},
};
use std::sync::Arc;

impl Executions {
    /// WorkHub owns the admission gate and supplies its exact delegated lineage.
    pub(crate) async fn resume_workhub(
        self: &Arc<Self>,
        origin: ResumeOrigin,
        owner: Invocation,
        connection: uuid::Uuid,
    ) -> Result<TurnResumeStartResult> {
        let session = self.resume_session(&owner.session_id).await?;
        if self
            .has_session_work(&owner.session_id)
            .await
            .map_err(internal)?
        {
            return Err(failure(
                Code::SessionBusy,
                "Delegated Session has pending or active work",
            ));
        }
        let source = match self
            .resume_source(&TurnResumeQueryInput {
                session_id: owner.session_id.clone(),
                source_run_id: Some(owner.run_id.clone()),
                expected_runtime_event_high_water: None,
            })
            .await?
        {
            Selection::Ready(source) if source.invocation == owner => source,
            _ => {
                return Err(failure(
                    Code::OperationConflict,
                    "Delegated execution has no safe resume boundary",
                ));
            }
        };
        let mut run = self
            .prepare_resume(
                &session,
                source,
                resumed_turn_id(&origin.action_id),
                Some(origin.request_fingerprint.clone()),
                prepare::Mode::Execute(connection),
            )
            .await?;
        let maka_agent::RunWork::Continuation { workhub_resume, .. } = &mut run.work else {
            unreachable!()
        };
        *workhub_resume = Some(origin);
        self.launch_resume(run).await
    }
}
