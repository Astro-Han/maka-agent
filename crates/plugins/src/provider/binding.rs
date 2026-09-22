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

use super::{Definition, Error};
use crate::{
    contributions::{Catalog, Contribution},
    fiber::CallGuard,
};

pub use maka_runtime::provider::Identity;

#[derive(Clone)]
pub struct Binding {
    identity: Identity,
    contribution: Contribution<Definition>,
}
impl Binding {
    pub fn resolve(identity: &Identity, catalog: &Catalog) -> Result<Self, Error> {
        identity.validate().map_err(Error::Invalid)?;
        let contribution = catalog
            .snapshot::<Definition>(&identity.scope)
            .entries
            .remove(&identity.name)
            .ok_or(Error::Unavailable)?;
        let binding = Self::new(identity.name.clone(), contribution)?;
        if binding.identity != *identity {
            return Err(Error::Unavailable);
        }
        Ok(binding)
    }

    pub fn new(name: String, contribution: Contribution<Definition>) -> Result<Self, Error> {
        let owner = contribution
            .owner
            .identity()
            .map_err(|_| Error::Unavailable)?;
        let identity = Identity {
            package_id: owner.package_id,
            entry_id: owner.entry_id,
            scope: owner.scope,
            name,
        };
        identity.validate().map_err(Error::Invalid)?;
        Ok(Self {
            identity,
            contribution,
        })
    }
    pub fn definition(&self) -> &Definition {
        &self.contribution.value
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub async fn prepare(&self, request: super::Resolve) -> Result<super::Model, Error> {
        self.definition()
            .validate_configuration(&request.connection.configuration)?;
        request.model.validate().map_err(Error::Invalid)?;
        let expected = request.model.id.clone();
        let _call = self.admit()?;
        let model = self.definition().implementation.resolve(request).await?;
        model.validate()?;
        if model.info.id != expected {
            return Err(Error::Invalid(
                "provider changed the selected model identity".into(),
            ));
        }
        Ok(model)
    }

    pub async fn authorize(
        &self,
        connection: super::Connection,
        credential: Option<super::authentication::Credential>,
        session_id: String,
    ) -> Result<crate::model::Credentials, Error> {
        self.definition()
            .validate_configuration(&connection.configuration)?;
        if let Some(credential) = &credential {
            credential.validate()?;
        }
        let _call = self.admit()?;
        let credentials = self
            .definition()
            .implementation
            .authorize(connection, credential, session_id)
            .await?;
        credentials.validate().map_err(Error::Invalid)?;
        Ok(credentials)
    }

    pub async fn authenticate(
        &self,
        request: super::authentication::Authenticate,
        context: super::Context,
    ) -> Result<super::authentication::Credential, Error> {
        self.definition()
            .validate_configuration(&request.connection.configuration)?;
        let validator = self
            .definition()
            .authentication
            .get(&request.method)
            .ok_or_else(|| Error::Invalid("unknown authentication method".into()))?;
        // Validation errors must never echo submitted credentials.
        if serde_json::to_vec(&request.input)
            .map_err(|_| Error::Invalid("invalid authentication input".into()))?
            .len()
            > 64 * 1024
            || !validator.is_valid(&request.input)
        {
            return Err(Error::Invalid(
                "authentication input does not satisfy its schema".into(),
            ));
        }
        let method = self
            .definition()
            .descriptor
            .authentication
            .iter()
            .find(|method| method.id == request.method)
            .expect("compiled authentication schema");
        if method.interactive && context.interaction.is_none() {
            return Err(Error::Unavailable);
        }
        let _call = self.admit()?;
        let credential = self
            .definition()
            .implementation
            .authenticate(request, context)
            .await?;
        credential.validate()?;
        Ok(credential)
    }

    pub async fn refresh(
        &self,
        connection: super::Connection,
        credential: super::authentication::Credential,
        context: super::Context,
    ) -> Result<super::authentication::Credential, Error> {
        self.definition()
            .validate_configuration(&connection.configuration)?;
        credential.validate()?;
        let _call = self.admit()?;
        let credential = self
            .definition()
            .implementation
            .refresh(connection, credential, context)
            .await?;
        credential.validate()?;
        Ok(credential)
    }

    pub async fn discover(
        &self,
        connection: super::Connection,
        credential: Option<super::authentication::Credential>,
        context: super::Context,
    ) -> Result<Vec<maka_runtime::configuration::ModelInfo>, Error> {
        if !self.definition().descriptor.discovery {
            return Err(Error::Unavailable);
        }
        self.definition()
            .validate_configuration(&connection.configuration)?;
        if let Some(credential) = &credential {
            credential.validate()?;
        }
        let _call = self.admit()?;
        let models = self
            .definition()
            .implementation
            .discover(connection, credential, context)
            .await?;
        if models.len() > 2048
            || serde_json::to_vec(&models)
                .map_err(|_| Error::Invalid("invalid model inventory".into()))?
                .len()
                > 4 * 1024 * 1024
        {
            return Err(Error::Invalid("model inventory exceeds its bounds".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for model in &models {
            model.validate().map_err(Error::Invalid)?;
            if !seen.insert(&model.id) {
                return Err(Error::Invalid("duplicate model identity".into()));
            }
        }
        Ok(models)
    }
    pub fn admit(&self) -> Result<CallGuard, Error> {
        self.contribution.admit().map_err(|_| Error::Unavailable)
    }
    pub fn source(&self) -> Result<maka_runtime::composition::SourceRevision, Error> {
        let owner = self
            .contribution
            .owner
            .identity()
            .map_err(|_| Error::Unavailable)?;
        Ok(maka_runtime::composition::SourceRevision {
            kind: maka_runtime::composition::SourceKind::ModelProvider,
            name: self.identity.name.clone(),
            package_id: owner.package_id,
            entry_id: owner.entry_id,
            activation: owner.activation,
            revision: self.contribution.registration_id().to_string(),
        })
    }
}
