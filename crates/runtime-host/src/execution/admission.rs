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
        self.ordinary_session(&input.session_id).await?;
        let fingerprint = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&input).map_err(internal)?)
        );
        let mut prepared = None;
        loop {
            let admission = self.lock_admission().await;
            if self.shutdown.is_cancelled() {
                return Err(failure(Code::HostDraining, "Host is draining"));
            }
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
            let Some(candidate) = prepared.take() else {
                drop(admission);
                prepared = Some(
                    async {
                        let environment = self
                            .prepare_environment(
                                &input.session_id,
                                Some(connection_id),
                                maka_client_capability::BindingMode::Strict,
                                input
                                    .turn_orchestration
                                    .as_ref()
                                    .map(|intent| intent.mode.clone()),
                            )
                            .await?;
                        environment
                            .expand(
                                input.content.clone().into(),
                                input.skill_ids.clone().unwrap_or_default(),
                            )
                            .await
                    }
                    .await,
                );
                continue;
            };
            let (environment, content, selection) = candidate?;
            let Some((environment, _input_admission)) =
                environment.commit(self, &input.session_id).await?
            else {
                continue;
            };
            let selection_result = match selection {
                super::skills::SkillPreparation::Ready {
                    skill_invocation, ..
                } => skill_invocation,
                super::skills::SkillPreparation::Blocked(skill_invocation) => {
                    return Ok(TurnStartResult::Blocked { skill_invocation });
                }
            };
            input.skill_ids = None;
            input.content = content.clone().into();
            let mut run = self
                .prepare_message(
                    input,
                    super::prepare::MessageOrigin::Client { root_id },
                    Some(fingerprint),
                    Vec::new(),
                    environment,
                )
                .await?;
            run.message(
                content,
                (!selection_result.is_empty()).then(|| Box::new(selection_result.clone())),
            )?;
            let turn = self.launch(run).await?;
            return Ok(TurnStartResult::Started {
                turn,
                skill_invocation: selection_result,
            });
        }
    }
}
