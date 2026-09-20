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

use super::protocol as operation;
use super::{BoundCommands, Error, storage};
use maka_plugins::execution::OfferInteraction;
use maka_runtime::interaction::{InteractionOutcome, InteractionRecord};
use sha2::{Digest, Sha256};

impl BoundCommands {
    fn interaction_id(&self, operation: &str) -> Result<String, Error> {
        if operation.is_empty()
            || operation.len() > 256
            || operation
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(Error::Invalid(
                "invalid interaction operation identity".into(),
            ));
        }
        let identity = serde_json::to_vec(&(
            self.namespace.package(),
            String::from(self.namespace.scope().clone()),
            operation,
        ))
        .map_err(|error| Error::Invalid(error.to_string()))?;
        Ok(format!("plugin-interaction-{:x}", Sha256::digest(identity)))
    }

    pub(super) async fn offer(&self, offer: OfferInteraction) -> Result<InteractionRecord, Error> {
        let request_id = self.interaction_id(&offer.operation_id)?;
        let request = offer
            .request(self.namespace.package())
            .map_err(|error| Error::Invalid(error.to_string()))?;
        let host = self.executions()?;
        let gate = host.interactions.own_admission().await;
        let lease = self.context.admit().map_err(|_| Error::Revoked)?;
        self.authorize(&host, &offer.invocation.session_id).await?;
        let cancellation = self.submission_stop.clone();
        let worker = host.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        // The Host, not a Remote request or plugin waiter, owns accepted publication.
        host.workers.spawn(async move {
            let result = worker
                .interactions
                .admit_stable_request(offer.invocation, request_id, request, &cancellation)
                .await
                .map_err(operation);
            drop(gate);
            drop(lease);
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| Error::OutcomeUnknown("interaction publisher disappeared".into()))?
    }

    pub(super) async fn read_interaction(
        &self,
        operation_id: String,
    ) -> Result<Option<InteractionRecord>, Error> {
        let request_id = self.interaction_id(&operation_id)?;
        let host = self.executions()?;
        let _lease = self.context.admit().map_err(|_| Error::Revoked)?;
        self.authorize_origin(&host).await?;
        let record = host.log.interaction(&request_id).await.map_err(storage)?;
        if let Some(record) = &record {
            self.authorize(&host, &record.session_id).await?;
        }
        Ok(record)
    }

    pub(super) async fn wait_for_interaction(
        &self,
        operation_id: String,
    ) -> Result<InteractionOutcome, Error> {
        let host = self.executions()?;
        let stopping = self.context.stopping().map_err(|_| Error::Revoked)?;
        let mut changes = host.log.subscribe_commits();
        loop {
            let record = self
                .read_interaction(operation_id.clone())
                .await?
                .ok_or(Error::NotFound)?;
            if let Some(outcome) = record.outcome {
                return Ok(outcome);
            }
            tokio::select! {
                _ = stopping.cancelled() => return Err(Error::Revoked),
                _ = self.submission_stop.cancelled() => return Err(Error::Revoked),
                _ = host.shutdown.cancelled() => return Err(Error::Draining),
                change = changes.changed() => { change.map_err(|_| Error::Draining)?; }
                // Consent/configuration changes need not append a canonical event.
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
            }
        }
    }

    pub(super) async fn withdraw_interaction(
        &self,
        operation_id: String,
    ) -> Result<InteractionRecord, Error> {
        let request_id = self.interaction_id(&operation_id)?;
        let host = self.executions()?;
        let gate = host.interactions.own_admission().await;
        let lease = self.context.admit().map_err(|_| Error::Revoked)?;
        self.authorize_origin(&host).await?;
        let record = host
            .log
            .interaction(&request_id)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        self.authorize(&host, &record.session_id).await?;
        let worker = host.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        host.workers.spawn(async move {
            let result = worker
                .interactions
                .withdraw(&request_id)
                .await
                .map_err(operation);
            drop(gate);
            drop(lease);
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| Error::OutcomeUnknown("interaction withdrawal owner disappeared".into()))?
    }
}
