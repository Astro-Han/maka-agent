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

use super::{Executions, Result, internal};
use std::sync::Arc;

impl Executions {
    /// Called before publishing readiness, after reconciling abandoned effects.
    /// Pending rows have no canonical delivery: this starts their accepted work,
    /// never an old model request or a dispatched tool with an unknown outcome.
    pub(crate) async fn recover_messages(self: &Arc<Self>) -> Result<()> {
        let mut after = None;
        loop {
            let sessions = self
                .log
                .pending_message_sessions(after.as_deref())
                .await
                .map_err(internal)?;
            if self.shutdown.is_cancelled() {
                return Err(super::failure(
                    maka_protocol::OperationErrorCode::HostDraining,
                    "Recovery is draining",
                ));
            }
            if sessions.is_empty() {
                return Ok(());
            }
            for session in &sessions {
                let _admission = self.lock_admission().await;
                if self.shutdown.is_cancelled() {
                    return Err(super::failure(
                        maka_protocol::OperationErrorCode::HostDraining,
                        "Recovery is draining",
                    ));
                }
                if !self.has_active_session(session)
                    && let Some((running, cancellation)) = self.next_message(session).await?
                {
                    self.track(running, cancellation);
                }
            }
            after = sessions.last().cloned();
        }
    }
}
