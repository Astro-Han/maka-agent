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

use crate::server::configuration::failure as configuration_error;
use crate::session::SessionModel;
use maka_config::{ConfigurationStore, model_catalog};
use maka_protocol::{
    OperationErrorCode,
    session::{SessionModelTarget, ThinkingLevel},
};
use maka_runtime::configuration::{
    ConnectionCredentialKind, ConnectionCredentialTarget, CredentialLocator, ProviderAuthKind,
};

pub(crate) async fn resolve(
    configuration: &ConfigurationStore,
    target: &SessionModelTarget,
    thinking: Option<ThinkingLevel>,
) -> Result<SessionModel, maka_protocol::OperationError> {
    let catalog = configuration.catalog().await.map_err(configuration_error)?;
    let (id, slug, model) = match target {
        SessionModelTarget::Default => {
            let target = catalog.default_target.as_ref().ok_or_else(|| {
                failure(
                    OperationErrorCode::OperationUnavailable,
                    "No default model is configured",
                )
            })?;
            (
                target.connection_id.as_str(),
                None,
                target.model_id.as_str(),
            )
        }
        SessionModelTarget::Explicit {
            connection_id,
            connection_slug,
            model,
        } => (
            connection_id.as_str(),
            Some(connection_slug.as_str()),
            model.as_str(),
        ),
    };
    let row = catalog
        .connections
        .iter()
        .find(|row| row.connection_id == id)
        .ok_or_else(|| {
            failure(
                if slug.is_some() {
                    OperationErrorCode::OperationConflict
                } else {
                    OperationErrorCode::InvalidRequest
                },
                "Model connection does not exist",
            )
        })?;
    if slug.is_some_and(|slug| slug != row.slug) {
        return Err(failure(
            OperationErrorCode::OperationConflict,
            "Bound model connection identity changed",
        ));
    }
    let facts = model_catalog::provider_facts(&row.provider_type).map_err(configuration_error)?;
    if !row.enabled || !row.enabled_model_ids.iter().any(|id| id == model) || facts.retired {
        return Err(failure(
            OperationErrorCode::InvalidRequest,
            "Model connection or model is not enabled",
        ));
    }
    let entries = model_catalog::resolve(row, Some(model)).map_err(configuration_error)?;
    let entry = entries
        .iter()
        .find(|entry| entry.id == model)
        .filter(|entry| entry.can_use_as_chat_default)
        .ok_or_else(|| {
            failure(
                OperationErrorCode::InvalidRequest,
                "Selected model cannot be used for chat",
            )
        })?;
    if let Some(level) = thinking
        && !entry.thinking_levels.contains(&level)
    {
        return Err(failure(
            OperationErrorCode::InvalidRequest,
            "Selected model does not support requested thinking level",
        ));
    }
    let kind = match facts.auth_kind {
        ProviderAuthKind::ApiKey => Some(ConnectionCredentialKind::ApiKey),
        ProviderAuthKind::OauthToken => Some(ConnectionCredentialKind::OauthToken),
        ProviderAuthKind::None | ProviderAuthKind::OptionalApiKey => None,
    };
    if let Some(kind) = kind {
        let target = ConnectionCredentialTarget {
            connection_id: row.connection_id.clone(),
            revision: row.revision,
            slug: row.slug.clone(),
            provider_type: row.provider_type.clone(),
            effective_base_url: row
                .base_url
                .clone()
                .unwrap_or_else(|| facts.base_url.clone()),
        };
        let secret = configuration
            .credential_secret(
                &CredentialLocator::Connection {
                    connection_id: row.connection_id.clone(),
                    kind,
                },
                Some(&target),
            )
            .await
            .map_err(configuration_error)?;
        if secret.is_none() {
            return Err(failure(
                OperationErrorCode::OperationUnavailable,
                "Model connection has no credentials",
            ));
        }
    }
    Ok(SessionModel {
        connection_id: row.connection_id.clone(),
        connection_slug: row.slug.clone(),
        model: model.into(),
    })
}

fn failure(code: OperationErrorCode, message: &str) -> maka_protocol::OperationError {
    maka_protocol::OperationError {
        code,
        message: message.into(),
    }
}
