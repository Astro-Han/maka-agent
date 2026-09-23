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

use super::{BoundCommands, Error, Executions, protocol, storage};
use maka_event_log::sessions::{RemovalAuthority, RemovalPlan, RemoveFamilyResult};
use maka_plugins::execution::{RemovalReceipt, RemoveSession, RemovedSession};

impl BoundCommands {
    pub(super) async fn read_removal_receipt(
        &self,
        session: String,
    ) -> Result<Option<RemovalReceipt>, Error> {
        let host = self.executions()?;
        let _lease = self.context.admit().map_err(|_| Error::Revoked)?;
        self.authorize_removal_receipt(&host, &session).await?;
        Ok(host
            .log
            .session_removal_receipt(&session)
            .await
            .map_err(storage)?
            .map(|plan| receipt(session, plan)))
    }

    async fn authorize_removal_receipt(
        &self,
        host: &Executions,
        session: &str,
    ) -> Result<(), Error> {
        self.authorize_origin(host).await?;
        if self.submission_stop.is_cancelled() {
            return Err(Error::Revoked);
        }
        if host
            .log
            .session_manager(session)
            .await
            .map_err(storage)?
            .is_some_and(|manager| manager != self.namespace)
        {
            return Err(Error::Denied);
        }
        if !self.grants.lock().unwrap().contains_key(session)
            && host
                .log
                .session_creator(session)
                .await
                .map_err(storage)?
                .as_ref()
                != Some(&self.namespace)
        {
            return Err(Error::Denied);
        }
        Ok(())
    }

    async fn authorize_removal_plan(
        &self,
        host: &Executions,
        session: &str,
    ) -> Result<RemovalPlan, Error> {
        self.authorize_session_control(host, session).await?;
        let plan = host
            .log
            .preview_session_removal(session)
            .await
            .map_err(storage)?;
        for target in &plan.remove {
            self.authorize_session_control(host, target).await?;
        }
        if self.submission_stop.is_cancelled() {
            return Err(Error::Revoked);
        }
        Ok(plan)
    }

    pub(super) async fn preview_session_removal(&self, session: String) -> Result<u64, Error> {
        let host = self.executions()?;
        let _lease = self.context.admit().map_err(|_| Error::Revoked)?;
        let _gate = host.interactions.own_admission().await;
        Ok(self
            .authorize_removal_plan(&host, &session)
            .await?
            .archived_subtask_count)
    }

    pub(super) async fn remove(&self, input: RemoveSession) -> Result<RemovedSession, Error> {
        input
            .validate()
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let host = self.executions()?;
        let _lease = self.context.admit().map_err(|_| Error::Revoked)?;
        let gate = host.interactions.own_admission().await;
        self.authorize_removal_receipt(&host, &input.session_id)
            .await?;
        if let Some(plan) = host
            .log
            .session_removal_receipt(&input.session_id)
            .await
            .map_err(storage)?
        {
            return Ok(RemovedSession::Removed {
                receipt: receipt(input.session_id, plan),
            });
        }
        let plan = self
            .authorize_removal_plan(&host, &input.session_id)
            .await?;
        let authority = RemovalAuthority::Granted {
            namespace: self.namespace.clone(),
            sessions: plan.remove.into_iter().collect(),
        };
        Ok(
            match host
                .remove_family(
                    input.session_id.clone(),
                    input.expected_revision,
                    authority,
                    gate,
                )
                .await
                .map_err(protocol)?
            {
                RemoveFamilyResult::Accepted(plan) => RemovedSession::Removed {
                    receipt: receipt(input.session_id, plan),
                },
                RemoveFamilyResult::RevisionConflict { expected, actual } => {
                    RemovedSession::RevisionConflict {
                        expected_revision: expected,
                        actual_revision: actual,
                    }
                }
            },
        )
    }
}

fn receipt(session_id: String, plan: RemovalPlan) -> RemovalReceipt {
    RemovalReceipt {
        session_id,
        archived_subtask_count: plan.archived_subtask_count,
    }
}
