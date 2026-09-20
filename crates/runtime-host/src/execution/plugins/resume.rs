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

use super::protocol;
use super::{BoundCommands, Error, storage};
use crate::execution::resume::{Selection, prepare::Mode};
use maka_plugins::execution::Resume;
use maka_runtime::event::Invocation;
use sha2::{Digest, Sha256};

impl BoundCommands {
    pub(super) async fn resume_owned(&self, request: Resume) -> Result<Invocation, Error> {
        let host = self.executions()?;
        let identity = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    "plugin-resume",
                    self.namespace.package(),
                    self.namespace.scope(),
                    &request.operation_id,
                ))
                .map_err(|e| Error::Invalid(e.to_string()))?
            )
        );
        let target = Invocation {
            session_id: request.source.session_id.clone(),
            turn_id: format!("resume-turn-{identity}"),
            run_id: format!("resume-run-{identity}"),
            invocation_id: format!("resume-inv-{identity}"),
        };
        let digest = format!(
            "sha256:{:x}",
            Sha256::digest(
                serde_json::to_vec(&request).map_err(|e| Error::Invalid(e.to_string()))?
            )
        );
        let mut prepared = None;
        loop {
            let gate = host.interactions.own_admission().await;
            self.authorize(&host, &request.source.session_id).await?;
            if let Some((invocation, source, fingerprint)) = host
                .log
                .continuation_receipt(&target.invocation_id)
                .await
                .map_err(storage)?
            {
                return if invocation == target
                    && source.invocation == request.source
                    && fingerprint == digest
                {
                    Ok(invocation)
                } else {
                    Err(Error::Conflict)
                };
            }
            if !host.accepting() {
                return Err(Error::Draining);
            }
            if self.submission_stop.is_cancelled() {
                return Err(Error::Revoked);
            }
            let session = host
                .resume_session(&request.source.session_id)
                .await
                .map_err(protocol)?;
            if host
                .has_session_work(&request.source.session_id)
                .await
                .map_err(storage)?
            {
                return Err(Error::Busy);
            }
            let source = match host
                .resume_source(&maka_protocol::turn::TurnResumeQueryInput {
                    session_id: request.source.session_id.clone(),
                    source_run_id: Some(request.source.run_id.clone()),
                    expected_runtime_event_high_water: None,
                })
                .await
                .map_err(protocol)?
            {
                Selection::Ready(source) if source.invocation == request.source => source,
                _ => return Err(Error::Conflict),
            };
            let Some(candidate) = prepared.take() else {
                drop(gate);
                prepared = Some(
                    async {
                        let environment = host
                            .prepare_environment(
                                &request.source.session_id,
                                self.call
                                    .as_ref()
                                    .and_then(|call| host.plugin_initiating_connection(call)),
                                maka_client_capability::BindingMode::Strict,
                                host.log
                                    .invocation_configuration(&source.invocation)
                                    .await
                                    .map_err(storage)?
                                    .map(|configuration| configuration.orchestration_mode),
                            )
                            .await
                            .map_err(protocol)?;
                        let mut run = host
                            .prepare_resume(
                                &session,
                                source,
                                target.turn_id.clone(),
                                Some(digest.clone()),
                                Mode::Prepared(&environment),
                            )
                            .await
                            .map_err(protocol)?;
                        run.invocation = target.clone();
                        let run = host
                            .engine
                            .prepare_continuation(run, &self.submission_stop)
                            .await
                            .map_err(|error| Error::Invalid(error.to_string()))?;
                        Ok::<_, Error>((environment, run))
                    }
                    .await,
                );
                continue;
            };
            let (environment, run) = candidate?;
            let Some((_environment, _surface)) = environment
                .commit(&host, &request.source.session_id)
                .await
                .map_err(protocol)?
            else {
                continue;
            };
            if self.submission_stop.is_cancelled() {
                return Err(Error::Revoked);
            }
            let running = host
                .engine
                .start_continuation(run, host.shutdown.child_token())
                .await
                .map_err(|error| {
                    if crate::execution::requires_drain(&error) {
                        host.begin_drain();
                    }
                    protocol(crate::execution::execution_error(error))
                })?;
            host.track(running);
            return Ok(target);
        }
    }
}
