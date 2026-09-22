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

mod catalog;
mod policy;

use maka_config::{ConfigError, ConfigurationStore};
use maka_protocol::OperationErrorCode;
use maka_protocol::configuration as wire;
use maka_protocol::request_headers;
use maka_protocol::{Operation, OperationError, ProtocolError, Result};
use maka_runtime::configuration::headers::{
    RequestHeadersQueryResult, RequestHeadersReplaceResult,
};
use maka_runtime::configuration::policy::{RuntimePolicyMutationResult, RuntimePolicySnapshot};
use maka_runtime::configuration::{
    CatalogMutationResult, ConnectionModelFetchResult, ConnectionTestRunResult,
    CredentialMutationResult, CredentialVaultQueryResult,
};
use serde_json::Value;

/// Preserve mutation evidence until the Host has decided which observers to wake.
/// JSON is only the wire representation, not a second commit authority.
#[derive(serde::Serialize)]
#[serde(untagged)]
pub(super) enum Output {
    Catalog(Value),
    Credential(CredentialVaultQueryResult),
    CatalogMutation(CatalogMutationResult),
    CredentialMutation(CredentialMutationResult),
    ModelFetch(ConnectionModelFetchResult),
    ConnectionTest(ConnectionTestRunResult),
    Policy(RuntimePolicySnapshot),
    PolicyMutation(RuntimePolicyMutationResult),
    NetworkProxyTest(maka_runtime::configuration::policy::network_test::Output),
    NetworkProxyMutation(maka_runtime::configuration::policy::network_update::UpdateResult),
    RequestHeaders(RequestHeadersQueryResult),
    RequestHeadersMutation(RequestHeadersReplaceResult),
}

impl Output {
    pub fn committed(&self) -> bool {
        matches!(
            self,
            Self::CatalogMutation(CatalogMutationResult::Committed { .. })
                | Self::CredentialMutation(CredentialMutationResult::Committed { .. })
                | Self::ModelFetch(ConnectionModelFetchResult::Committed { .. })
                | Self::ConnectionTest(ConnectionTestRunResult::Committed { .. })
                | Self::PolicyMutation(RuntimePolicyMutationResult::Committed { .. })
                | Self::NetworkProxyMutation(
                    maka_runtime::configuration::policy::network_update::UpdateResult::Committed { .. }
                )
                | Self::RequestHeadersMutation(RequestHeadersReplaceResult::Committed { .. })
        )
    }
}

pub(super) fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::ModelProviderCatalogQuery
            | Operation::ExecutorCatalogQuery
            | Operation::ConnectionCatalogQuery
            | Operation::ConnectionRequestHeadersQuery
            | Operation::ConnectionRequestHeadersReplace
            | Operation::RuntimePolicyQuery
            | Operation::RuntimePolicyMutate
            | Operation::RuntimePolicyNetworkProxyUpdate
            | Operation::NetworkProxyTest
            | Operation::ConnectionModelsFetch
            | Operation::ConnectionTestRun
            | Operation::ConnectionCatalogCreate
            | Operation::ConnectionCatalogUpdate
            | Operation::ConnectionCatalogRemove
            | Operation::ConnectionCatalogSetDefaultTarget
            | Operation::CredentialVaultQuery
            | Operation::CredentialVaultSet
            | Operation::CredentialVaultDelete
    )
}

pub(super) fn decode_input(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::ExecutorCatalogQuery => {
            let query = serde_json::from_value::<maka_plugins::executor::Query>(value.clone())
                .map_err(|error| ProtocolError::invalid(error.to_string()))?;
            maka_plugins::executor::Search { query: query.query }
                .validate()
                .map_err(|error| ProtocolError::invalid(error.to_string()))?;
        }
        Operation::ModelProviderCatalogQuery => {
            serde_json::from_value::<maka_plugins::provider::catalog::Query>(value.clone())
                .map_err(|_| ProtocolError::invalid("Invalid model provider query"))?;
        }
        Operation::NetworkProxyTest => {
            maka_protocol::network_proxy::decode_input(value)?;
        }
        Operation::RuntimePolicyNetworkProxyUpdate => {
            maka_protocol::runtime_policy::decode_network_proxy_update(value)?;
        }
        Operation::ConnectionRequestHeadersQuery => {
            request_headers::decode_query(value)?;
        }
        Operation::ConnectionRequestHeadersReplace => {
            request_headers::decode_replace(value)?;
        }
        Operation::RuntimePolicyQuery => {
            maka_protocol::runtime_policy::decode_query_input(value)?;
        }
        Operation::RuntimePolicyMutate => {
            maka_protocol::runtime_policy::decode_mutation_input(value)?;
        }
        Operation::ConnectionTestRun => {
            maka_protocol::connection_effects::decode_connection_test_run_input(value)?;
        }
        Operation::ConnectionModelsFetch => {
            maka_protocol::connection_effects::decode_connection_model_fetch_input(value)?;
        }
        Operation::ConnectionCatalogQuery => {
            wire::decode_catalog_query_input(value)?;
        }
        Operation::ConnectionCatalogCreate => {
            wire::decode_create_connection_input(value)?;
        }
        Operation::ConnectionCatalogUpdate => {
            wire::decode_update_connection_input(value)?;
        }
        Operation::ConnectionCatalogRemove => {
            wire::decode_remove_connection_input(value)?;
        }
        Operation::ConnectionCatalogSetDefaultTarget => {
            wire::decode_set_default_target_input(value)?;
        }
        Operation::CredentialVaultQuery => {
            wire::decode_credential_query_input(value)?;
        }
        Operation::CredentialVaultSet => {
            wire::decode_set_credential_input(value)?;
        }
        Operation::CredentialVaultDelete => {
            wire::decode_delete_credential_input(value)?;
        }
        _ => return Err(ProtocolError::invalid("Unknown configuration operation")),
    }
    Ok(value.clone())
}

pub(super) fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::ExecutorCatalogQuery => {
            serde_json::from_value::<maka_plugins::executor::Choices>(value.clone())
                .map_err(|error| ProtocolError::invalid(error.to_string()))?;
        }
        Operation::ModelProviderCatalogQuery => {
            serde_json::from_value::<maka_plugins::provider::catalog::Page>(value.clone())
                .map_err(|_| ProtocolError::invalid("Invalid model provider directory"))?;
        }
        Operation::NetworkProxyTest => {
            maka_protocol::network_proxy::decode_output(value)?;
        }
        Operation::RuntimePolicyNetworkProxyUpdate => {
            maka_protocol::runtime_policy::decode_network_proxy_result(value)?;
        }
        Operation::ConnectionRequestHeadersQuery => {
            request_headers::decode_query_result(value)?;
        }
        Operation::ConnectionRequestHeadersReplace => {
            request_headers::decode_replace_result(value)?;
        }
        Operation::RuntimePolicyQuery => {
            maka_protocol::runtime_policy::decode_query_result(value)?;
        }
        Operation::RuntimePolicyMutate => {
            maka_protocol::runtime_policy::decode_mutation_result(value)?;
        }
        Operation::ConnectionTestRun => {
            maka_protocol::connection_effects::decode_connection_test_run_result(value)?;
        }
        Operation::ConnectionModelsFetch => {
            maka_protocol::connection_effects::decode_connection_model_fetch_result(value)?;
        }
        Operation::ConnectionCatalogQuery => {
            maka_protocol::configuration_pages::decode_catalog_query_result(value)?;
        }
        Operation::CredentialVaultQuery => {
            wire::decode_credential_query_result(value)?;
        }
        Operation::CredentialVaultSet | Operation::CredentialVaultDelete => {
            wire::decode_credential_mutation_result(operation, value)?;
        }
        _ => {
            wire::decode_catalog_mutation_result(operation, value)?;
        }
    }
    Ok(value.clone())
}

pub(super) async fn execute(
    host: &super::Host,
    operation: Operation,
    value: &Value,
) -> std::result::Result<Output, OperationError> {
    if operation == Operation::ExecutorCatalogQuery {
        let input: maka_plugins::executor::Query = serde_json::from_value(value.clone())
            .map_err(|error| failure(maka_config::ConfigError::Json(error)))?;
        let result = maka_plugins::executor::search(
            &host.executions.plugin_catalog,
            &input.scope,
            maka_plugins::executor::Search { query: input.query },
        )
        .map_err(|error| failure(maka_config::ConfigError::Invalid(error.to_string())))?;
        return serde_json::to_value(result)
            .map(Output::Catalog)
            .map_err(|error| failure(maka_config::ConfigError::Json(error)));
    }
    if operation == Operation::ModelProviderCatalogQuery {
        let input = serde_json::from_value(value.clone()).map_err(|_| {
            failure(maka_config::ConfigError::Invalid(
                "invalid provider query".into(),
            ))
        })?;
        let result = maka_plugins::provider::catalog::query(&host.executions.plugin_catalog, input)
            .map_err(|error| failure(maka_config::ConfigError::Invalid(error.to_string())))?;
        return serde_json::to_value(result)
            .map(Output::Catalog)
            .map_err(|error| failure(maka_config::ConfigError::Json(error)));
    }
    if operation == Operation::NetworkProxyTest {
        return policy::test_network(&host.configuration, value).await;
    }
    if operation == Operation::ConnectionCatalogQuery {
        return catalog::query(host, value).await;
    }
    if matches!(
        operation,
        Operation::RuntimePolicyQuery | Operation::RuntimePolicyMutate
    ) {
        return policy::execute(&host.configuration, operation, value).await;
    }
    if operation == Operation::ConnectionTestRun {
        let input =
            parsed(maka_protocol::connection_effects::decode_connection_test_run_input(value))
                .map_err(failure)?;
        return host
            .connection_effects
            .test(host, input)
            .await
            .map(Output::ConnectionTest);
    }
    if operation == Operation::ConnectionModelsFetch {
        let input =
            parsed(maka_protocol::connection_effects::decode_connection_model_fetch_input(value))
                .map_err(failure)?;
        return host
            .connection_effects
            .fetch(host, &input.connection_id)
            .await
            .map(Output::ModelFetch);
    }
    let removal = if operation == Operation::ConnectionCatalogRemove {
        Some(parsed(wire::decode_remove_connection_input(value)).map_err(failure)?)
    } else {
        None
    };
    let _admission = if removal.is_some() {
        Some(host.executions.lock_admission().await)
    } else {
        None
    };
    let output = execute_store(&host.configuration, operation, value)
        .await
        .map_err(failure)?;
    if output.committed()
        && let Some(removal) = removal
    {
        host.executions
            .oauth
            .forget_connection(&removal.expected.connection_id);
    }
    Ok(output)
}

async fn execute_store(
    store: &ConfigurationStore,
    operation: Operation,
    value: &Value,
) -> maka_config::Result<Output> {
    let result = match operation {
        Operation::RuntimePolicyNetworkProxyUpdate => Output::NetworkProxyMutation(
            store
                .update_network_proxy(
                    parsed(maka_protocol::runtime_policy::decode_network_proxy_update(
                        value,
                    ))?,
                    now()?,
                )
                .await?,
        ),
        Operation::ConnectionRequestHeadersQuery => Output::RequestHeaders(
            store
                .request_headers(parsed(request_headers::decode_query(value))?.connection_id)
                .await?,
        ),
        Operation::ConnectionRequestHeadersReplace => Output::RequestHeadersMutation(
            store
                .replace_request_headers(parsed(request_headers::decode_replace(value))?, now()?)
                .await?,
        ),
        Operation::ConnectionCatalogCreate => Output::CatalogMutation(
            store
                .create_connection(parsed(wire::decode_create_connection_input(value))?)
                .await?,
        ),
        Operation::ConnectionCatalogUpdate => Output::CatalogMutation(
            store
                .update_connection(parsed(wire::decode_update_connection_input(value))?)
                .await?,
        ),
        Operation::ConnectionCatalogRemove => Output::CatalogMutation(
            store
                .remove_connection(parsed(wire::decode_remove_connection_input(value))?)
                .await?,
        ),
        Operation::ConnectionCatalogSetDefaultTarget => Output::CatalogMutation(
            store
                .set_default_target(parsed(wire::decode_set_default_target_input(value))?)
                .await?,
        ),
        Operation::CredentialVaultQuery => Output::Credential(
            store
                .credential_status(parsed(wire::decode_credential_query_input(value))?.locator)
                .await?,
        ),
        Operation::CredentialVaultSet => Output::CredentialMutation(
            store
                .set_credential(parsed(wire::decode_set_credential_input(value))?, now()?)
                .await?,
        ),
        Operation::CredentialVaultDelete => Output::CredentialMutation(
            store
                .delete_credential(parsed(wire::decode_delete_credential_input(value))?)
                .await?,
        ),
        _ => {
            return Err(ConfigError::Invalid(
                "unknown configuration operation".into(),
            ));
        }
    };
    Ok(result)
}

fn parsed<T>(value: Result<T>) -> maka_config::Result<T> {
    value.map_err(|error| ConfigError::Invalid(error.message))
}

pub(super) fn now() -> maka_config::Result<u64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| ConfigError::Invalid(error.to_string()))?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| ConfigError::Invalid("clock exceeds timestamp range".into()))
}

pub(crate) fn failure(error: ConfigError) -> OperationError {
    let code = match &error {
        ConfigError::Invalid(_) => OperationErrorCode::InvalidRequest,
        ConfigError::CommitUnknown => OperationErrorCode::CommitOutcomeUnknown,
        _ => OperationErrorCode::PersistenceFailed,
    };
    OperationError {
        code,
        message: error.to_string().chars().take(1024).collect(),
    }
}

pub(super) const QUERY_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::InternalFailure,
    OperationErrorCode::PersistenceFailed,
];
pub(super) const MUTATION_ERRORS: &[OperationErrorCode] = &[
    OperationErrorCode::HostNotReady,
    OperationErrorCode::HostDraining,
    OperationErrorCode::OperationUnavailable,
    OperationErrorCode::InvalidRequest,
    OperationErrorCode::InternalFailure,
    OperationErrorCode::PersistenceFailed,
    OperationErrorCode::CommitOutcomeUnknown,
];
