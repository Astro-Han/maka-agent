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

use super::{Host, OperationError};
use maka_config::{network::NetworkConfiguration, oauth::ProviderCredential};
use maka_plugins::provider::{Binding, Connection, Context, Discovery, Error, Verification};
use maka_runtime::configuration::{
    ConnectionCatalogEntry, ConnectionEffectFailureClass as Failure, ModelInfo,
};
use std::time::Duration;

pub(in crate::server) struct ProviderOperation {
    pub binding: Binding,
    pub connection: Connection,
    pub context: Context,
    credential: Option<crate::oauth::Credential>,
}

pub(super) struct TestFailure {
    pub class: Failure,
    pub status: Option<u16>,
}
impl From<Failure> for TestFailure {
    fn from(class: Failure) -> Self {
        Self {
            class,
            status: None,
        }
    }
}
impl From<Error> for TestFailure {
    fn from(error: Error) -> Self {
        let status = if let Error::Http(status) = &error {
            (100..600).contains(status).then_some(*status)
        } else {
            None
        };
        Self {
            class: classify(error),
            status,
        }
    }
}

impl ProviderOperation {
    /// Caller holds the Host admission gate; callbacks run only after release.
    pub fn prepare(
        host: &Host,
        row: &ConnectionCatalogEntry,
        snapshot: Option<ProviderCredential>,
        network: &NetworkConfiguration,
    ) -> Result<Self, OperationError> {
        let binding =
            Binding::resolve(&row.provider, &host.executions.plugin_catalog).map_err(failure)?;
        let policy =
            maka_network::Policy::from_host_settings(&network.proxy, network.password.as_deref())
                .map_err(failure)?;
        let credential = snapshot
            .map(|snapshot| host.executions.oauth.bind(snapshot))
            .transpose()
            .map_err(failure)?;
        Ok(Self {
            binding,
            connection: Connection {
                id: row.connection_id.clone(),
                revision: row.revision,
                configuration: row.configuration.clone(),
            },
            context: Context {
                transport: host.executions.model_transport(&policy).map_err(failure)?,
                cancellation: host.draining.child_token(),
                interaction: None,
            },
            credential,
        })
    }

    pub async fn credential(&self) -> Result<Option<ProviderCredential>, OperationError> {
        match &self.credential {
            Some(credential) => credential
                .resolve(self.binding.clone(), self.context.clone())
                .await
                .map(Some)
                .map_err(failure),
            None => Ok(None),
        }
    }

    pub async fn discover(
        &self,
        credential: Option<&ProviderCredential>,
        headers: Option<&str>,
    ) -> Result<Vec<ModelInfo>, Failure> {
        self.discover_with(credential.map(|c| c.credential().clone()), headers)
            .await
    }

    pub async fn discover_with(
        &self,
        credential: Option<maka_runtime::provider::Credential>,
        headers: Option<&str>,
    ) -> Result<Vec<ModelInfo>, Failure> {
        let request = Discovery {
            connection: self.connection.clone(),
            credential,
            request_headers: parse_headers(headers)?,
        };
        let context = Context {
            cancellation: self.context.cancellation.child_token(),
            ..self.context.clone()
        };
        let _cancel = context.cancellation.clone().drop_guard();
        tokio::time::timeout(
            Duration::from_secs(45),
            self.binding.discover(request, context),
        )
        .await
        .map_err(|_| Failure::Timeout)?
        .map_err(classify)
    }

    pub(super) async fn verify(
        &self,
        row: &ConnectionCatalogEntry,
        model: ModelInfo,
        credential: Option<&ProviderCredential>,
        headers: Option<&str>,
    ) -> Result<(), TestFailure> {
        let overrides = row
            .model_overrides
            .as_ref()
            .and_then(|values| values.get(&model.id))
            .cloned();
        let request = Verification {
            connection: self.connection.clone(),
            credential: credential.map(|c| c.credential().clone()),
            request_headers: parse_headers(headers)?,
            model,
            overrides,
            request_body_overlay: row.request_body_overlay.clone(),
        };
        let context = Context {
            cancellation: self.context.cancellation.child_token(),
            ..self.context.clone()
        };
        let _cancel = context.cancellation.clone().drop_guard();
        tokio::time::timeout(
            Duration::from_secs(45),
            self.binding.verify(request, context),
        )
        .await
        .map_err(|_| Failure::Timeout)?
        .map_err(TestFailure::from)
    }
}

fn parse_headers(
    headers: Option<&str>,
) -> Result<std::collections::BTreeMap<String, String>, Failure> {
    headers
        .map(maka_runtime::configuration::validation::parse_headers)
        .transpose()
        .map_err(|_| Failure::InvalidResponse)
        .map(Option::unwrap_or_default)
}

pub(super) fn classify(error: Error) -> Failure {
    match error {
        Error::AuthenticationRequired => Failure::Auth,
        Error::Unavailable => Failure::ProviderUnavailable,
        Error::Transport(_) => Failure::Network,
        Error::Http(status) => match status {
            401 | 403 => Failure::Auth,
            408 => Failure::Timeout,
            429 | 500..=599 => Failure::ProviderUnavailable,
            _ => Failure::Unknown,
        },
        Error::Invalid(_) => Failure::InvalidResponse,
        Error::Cancelled | Error::OutcomeUnknown | Error::Rejected(_) => Failure::Unknown,
    }
}

fn failure(error: impl std::fmt::Display) -> OperationError {
    OperationError {
        code: maka_protocol::OperationErrorCode::OperationUnavailable,
        message: error.to_string().chars().take(1024).collect(),
    }
}
