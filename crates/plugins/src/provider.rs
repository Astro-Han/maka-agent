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

//! Provider contributions compose authentication, discovery and model adapters.
//! Credentials and connections belong to Host, not to the contribution catalog.
pub mod authentication;
mod binding;
pub mod catalog;
pub mod configuration;

use crate::model::{Credentials, Transport};
use authentication::{Authenticate, Credential, Interaction};
pub use binding::{Binding, Identity};
pub use configuration::{Connection, Descriptor, Model, Resolve};
use futures_util::future::BoxFuture;
use maka_runtime::configuration::ModelInfo;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Context {
    pub transport: Arc<dyn Transport>,
    pub cancellation: CancellationToken,
    /// Present only for a Host-admitted interactive login.
    pub interaction: Option<Arc<dyn Interaction>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum Error {
    #[error("provider registration is unavailable")]
    Unavailable,
    #[error("provider operation was cancelled")]
    Cancelled,
    #[error("provider requires authentication")]
    AuthenticationRequired,
    #[error("provider rejected the request: {0}")]
    Rejected(String),
    #[error("invalid provider input or output: {0}")]
    Invalid(String),
    #[error("provider request outcome is unknown")]
    OutcomeUnknown,
    #[error("provider transport failed: {0}")]
    Transport(String),
}

/// Pure request policy is separate from credential resolution. Host can freeze
/// policy without disclosing tokens; authorization follows execution admission.
pub trait Provider: Send + Sync {
    fn resolve(&self, request: Resolve) -> BoxFuture<'_, Result<Model, Error>>;

    fn authorize(
        &self,
        connection: Connection,
        credential: Option<Credential>,
        session_id: String,
    ) -> BoxFuture<'_, Result<Credentials, Error>>;

    fn authenticate(
        &self,
        _request: Authenticate,
        _context: Context,
    ) -> BoxFuture<'_, Result<Credential, Error>> {
        Box::pin(async { Err(Error::AuthenticationRequired) })
    }

    fn refresh(
        &self,
        _connection: Connection,
        _credential: Credential,
        _context: Context,
    ) -> BoxFuture<'_, Result<Credential, Error>> {
        Box::pin(async { Err(Error::AuthenticationRequired) })
    }

    fn discover(
        &self,
        _connection: Connection,
        _credential: Option<Credential>,
        _context: Context,
    ) -> BoxFuture<'_, Result<Vec<ModelInfo>, Error>> {
        Box::pin(async { Err(Error::Unavailable) })
    }
}

pub struct Definition {
    descriptor: Descriptor,
    implementation: Arc<dyn Provider>,
    configuration: jsonschema::Validator,
    authentication: std::collections::BTreeMap<String, jsonschema::Validator>,
}

impl Definition {
    pub fn new(descriptor: Descriptor, implementation: Arc<dyn Provider>) -> Result<Self, Error> {
        descriptor.validate()?;
        let compile = |schema: &serde_json::Value| {
            jsonschema::options()
                .offline()
                .should_validate_formats(false)
                .build(schema)
                .map_err(|_| Error::Invalid("invalid provider schema".into()))
        };
        let configuration = compile(&descriptor.configuration_schema)?;
        let authentication = descriptor
            .authentication
            .iter()
            .map(|method| Ok((method.id.clone(), compile(&method.input_schema)?)))
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            descriptor,
            implementation,
            configuration,
            authentication,
        })
    }

    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }

    /// Defaults are materialized once, when creating the connection. Reading a
    /// stored connection never substitutes a newer plugin's endpoint or defaults.
    pub fn configure(&self, input: serde_json::Value) -> Result<serde_json::Value, Error> {
        let serde_json::Value::Object(input) = input else {
            return Err(Error::Invalid(
                "provider configuration must be an object".into(),
            ));
        };
        let mut configuration = self
            .descriptor
            .configuration_defaults
            .as_object()
            .expect("validated descriptor")
            .clone();
        configuration.extend(input);
        let configuration = serde_json::Value::Object(configuration);
        self.validate_configuration(&configuration)?;
        Ok(configuration)
    }

    fn validate_configuration(&self, configuration: &serde_json::Value) -> Result<(), Error> {
        if serde_json::to_vec(configuration)
            .map_err(|_| Error::Invalid("invalid configuration".into()))?
            .len()
            > 64 * 1024
            || !self.configuration.is_valid(configuration)
        {
            return Err(Error::Invalid(
                "provider configuration does not satisfy its schema".into(),
            ));
        }
        Ok(())
    }
}
