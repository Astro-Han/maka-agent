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

use super::{Executions, Result, failure, internal};
use maka_protocol::OperationErrorCode as Code;
use maka_runtime::event::Invocation;

impl Executions {
    pub(crate) fn active_session_owner(&self, session: &str) -> Option<Invocation> {
        self.active
            .lock()
            .unwrap()
            .values()
            .find(|run| run.invocation.session_id == session && !run.cancellation.is_cancelled())
            .map(|run| run.invocation.clone())
    }

    /// Caller owns admission; wait for cleanup only after releasing it.
    pub(crate) async fn retire_owner(
        &self,
        owner: &Invocation,
    ) -> Result<Option<tokio_util::sync::CancellationToken>> {
        let stored = |error: maka_event_log::StoreError| {
            if matches!(
                error,
                maka_event_log::StoreError::CommitUnknown(_)
                    | maka_event_log::StoreError::OperationUnknown
            ) {
                self.begin_drain();
                failure(Code::CommitOutcomeUnknown, &error.to_string())
            } else {
                internal(error)
            }
        };
        let boundary = self.log.handoff_owner(owner).await.map_err(stored)?;
        let boundary = if matches!(
            boundary.state,
            maka_event_log::turns::InvocationState::Ended {
                outcome: maka_runtime::event::InvocationOutcome::HandoffPaused { .. },
                ..
            }
        ) {
            self.log
                .cancel_handoff(&boundary.invocation)
                .await
                .map_err(stored)?
        } else {
            boundary
        };
        let owner = &boundary.invocation;
        let active = self
            .active
            .lock()
            .unwrap()
            .get(&owner.run_id)
            .filter(|run| run.invocation == *owner)
            .cloned();
        let Some(active) = active else {
            return Ok(None);
        };
        let stopped = self.interactions.stop_run(owner).await;
        active.cancellation.cancel();
        stopped?;
        Ok(Some(active.completed))
    }
}
