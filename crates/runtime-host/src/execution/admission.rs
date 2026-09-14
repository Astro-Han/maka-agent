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
use maka_protocol::{OperationErrorCode as Code, turn::*};
use sha2::{Digest, Sha256};
use uuid::Uuid;

impl Executions {
    pub(crate) async fn start(
        self: &std::sync::Arc<Self>,
        mut input: TurnStartInput,
        connection_id: Uuid,
        root_id: &str,
    ) -> Result<TurnStartResult> {
        super::ordinary_session(&input.session_id)?;
        let _admission = self.lock_admission().await;
        if self.shutdown.is_cancelled() {
            return Err(failure(Code::HostDraining, "Host is draining"));
        }
        let fingerprint = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&input).map_err(internal)?)
        );
        if let Some(record) = self.recorded(&input.session_id, &input.turn_id).await? {
            if record.fingerprint.as_deref() != Some(&fingerprint) {
                return Err(failure(
                    Code::OperationConflict,
                    "Turn identity belongs to another request",
                ));
            }
            return Ok(TurnStartResult::Started {
                turn: record.snapshot,
                skill_invocation: record.skill_invocation,
            });
        }
        if self
            .active
            .lock()
            .unwrap()
            .values()
            .any(|run| run.invocation.session_id == input.session_id)
        {
            return Err(failure(Code::SessionBusy, "Session has an active Run"));
        }
        if !self
            .log
            .pending_messages(&input.session_id)
            .await
            .map_err(internal)?
            .is_empty()
        {
            return Err(failure(Code::SessionBusy, "Session has pending Messages"));
        }
        let skill_ids = input.skill_ids.take().unwrap_or_default();
        let (mut run, skills) = self
            .prepare_message(
                input,
                super::prepare::MessageOrigin::Client {
                    connection_id,
                    root_id,
                },
                Some(fingerprint),
                Vec::new(),
            )
            .await?;
        let maka_agent::RunWork::Message {
            message,
            skill_invocation,
            ..
        } = &mut run.work
        else {
            return Err(internal("Message preparation produced a non-message Run"));
        };
        match skills.prepare(message, &skill_ids)? {
            super::skills::SkillPreparation::Ready {
                skill_invocation: result,
                ..
            } => {
                *skill_invocation = (!result.is_empty()).then(|| Box::new(result));
            }
            super::skills::SkillPreparation::Blocked(skill_invocation) => {
                return Ok(TurnStartResult::Blocked { skill_invocation });
            }
        }
        let skill_invocation = skill_invocation.as_deref().cloned().unwrap_or_default();
        let turn = self.launch(run).await?;
        Ok(TurnStartResult::Started {
            turn,
            skill_invocation,
        })
    }
}
