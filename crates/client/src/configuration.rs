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

use crate::{Client, ClientError, RequestFailure};
use maka_protocol::configuration::onboarding::{
    OnboardingInput, OnboardingSaveResult, OnboardingTarget, OnboardingVerifyResult,
};
use maka_protocol::configuration::{
    CatalogMutationResult, ConnectionVersionBasis, RemoveCatalogConnectionInput,
    SetDefaultConnectionTargetInput, UpdateCatalogConnectionInput,
};
use maka_protocol::{
    Operation, ProtocolError,
    configuration::{ConnectionCatalogCursor, ConnectionCatalogQueryInput},
};
use serde_json::Value;

impl Client {
    pub async fn test_connection(
        &self,
        input: maka_protocol::connection_effects::ConnectionTestRunInput,
    ) -> Result<maka_protocol::connection_effects::ConnectionTestRunResult, RequestFailure> {
        use maka_protocol::connection_effects::{
            ConnectionTestProjection, ConnectionTestRunResult, decode_connection_test_run_result,
        };
        let value = self
            .request(
                Operation::ConnectionTestRun,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let invalid = || {
            self.disconnect();
            RequestFailure::Unknown(ClientError::Protocol(
                "Connection test result does not match request".into(),
            ))
        };
        let result = decode_connection_test_run_result(&value).map_err(|_| invalid())?;
        if let ConnectionTestRunResult::Committed {
            connection, test, ..
        } = &result
        {
            let observed = match test {
                ConnectionTestProjection::Verified { model_id, .. } => Some(model_id),
                ConnectionTestProjection::Failed { model_id, .. } => model_id.as_ref(),
            };
            if connection.connection_id != input.connection_id
                || matches!((&input.model_id, observed), (Some(expected), Some(actual)) if expected.trim() != actual)
            {
                return Err(invalid());
            }
        }
        Ok(result)
    }
    pub async fn fetch_connection_models(
        &self,
        connection_id: &str,
    ) -> Result<maka_protocol::connection_effects::ConnectionModelFetchResult, RequestFailure> {
        use maka_protocol::connection_effects::{
            ConnectionModelFetchResult, decode_connection_model_fetch_result,
        };
        let value = self
            .request(
                Operation::ConnectionModelsFetch,
                serde_json::json!({"connectionId":connection_id}),
            )
            .await?;
        let invalid = || {
            self.disconnect();
            RequestFailure::Unknown(ClientError::Protocol(
                "Model refresh result does not match connection".into(),
            ))
        };
        let result = decode_connection_model_fetch_result(&value).map_err(|_| invalid())?;
        if matches!(&result,ConnectionModelFetchResult::Committed{connection,..} if connection.connection_id!=connection_id)
        {
            return Err(invalid());
        }
        Ok(result)
    }
    pub async fn set_default_model(
        &self,
        input: SetDefaultConnectionTargetInput,
    ) -> Result<CatalogMutationResult, RequestFailure> {
        let value = self
            .request(
                Operation::ConnectionCatalogSetDefaultTarget,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let invalid = |error: ProtocolError| {
            self.disconnect();
            RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
        };
        let result = maka_protocol::configuration::decode_catalog_mutation_result(
            Operation::ConnectionCatalogSetDefaultTarget,
            &value,
        )
        .map_err(invalid)?;
        let valid = match &result {
            CatalogMutationResult::Committed {
                catalog_revision, ..
            } => *catalog_revision == input.expected_catalog_revision + 1,
            CatalogMutationResult::RevisionConflict {
                expected_revision,
                actual_revision,
            } => {
                *expected_revision == input.expected_catalog_revision
                    && actual_revision != expected_revision
            }
            CatalogMutationResult::InvalidDefaultTarget { target } => {
                Some(target) == input.target.as_ref()
            }
            _ => false,
        };
        if !valid {
            return Err(invalid(ProtocolError::invalid(
                "Default model result does not match request",
            )));
        }
        Ok(result)
    }
    pub async fn update_connection(
        &self,
        input: UpdateCatalogConnectionInput,
    ) -> Result<CatalogMutationResult, RequestFailure> {
        self.mutate_connection(
            Operation::ConnectionCatalogUpdate,
            &input.expected,
            serde_json::to_value(&input).expect("wire input"),
        )
        .await
    }
    pub async fn remove_connection(
        &self,
        input: RemoveCatalogConnectionInput,
    ) -> Result<CatalogMutationResult, RequestFailure> {
        self.mutate_connection(
            Operation::ConnectionCatalogRemove,
            &input.expected,
            serde_json::to_value(&input).expect("wire input"),
        )
        .await
    }
    async fn mutate_connection(
        &self,
        operation: Operation,
        expected: &ConnectionVersionBasis,
        input: Value,
    ) -> Result<CatalogMutationResult, RequestFailure> {
        let value = self.request(operation, input).await?;
        let invalid = |error: ProtocolError| {
            self.disconnect();
            RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
        };
        let result =
            maka_protocol::configuration::decode_catalog_mutation_result(operation, &value)
                .map_err(invalid)?;
        let valid = match &result {
            CatalogMutationResult::Committed {
                connection: Some(actual),
                ..
            } => {
                actual.connection_id == expected.connection_id
                    && actual.revision == expected.revision + 1
            }
            CatalogMutationResult::Committed {
                connection: None, ..
            } => operation == Operation::ConnectionCatalogRemove,
            CatalogMutationResult::ConnectionStale {
                expected: echo,
                actual,
            } => {
                echo == expected
                    && actual.as_ref().is_none_or(|actual| {
                        actual.connection_id == expected.connection_id
                            && actual.revision != expected.revision
                    })
            }
            _ => false,
        };
        if !valid {
            return Err(invalid(ProtocolError::invalid(
                "Connection mutation result does not match request",
            )));
        }
        Ok(result)
    }
    pub async fn verify_connection(
        &self,
        input: OnboardingInput,
    ) -> Result<OnboardingVerifyResult, RequestFailure> {
        let value = self
            .request(
                Operation::ConnectionOnboardingVerify,
                onboarding_value(input),
            )
            .await?;
        let invalid = |e: ProtocolError| {
            self.disconnect();
            RequestFailure::Unknown(ClientError::Protocol(e.to_string()))
        };
        let result = maka_protocol::onboarding::decode_verify_result(&value).map_err(invalid)?;
        if let OnboardingVerifyResult::Verified { models } = &result {
            let unique: std::collections::HashSet<_> = models.iter().map(|m| &m.id).collect();
            if models.len() > 2048 || unique.len() != models.len() {
                return Err(invalid(ProtocolError::invalid(
                    "Invalid onboarding model inventory",
                )));
            }
        }
        Ok(result)
    }
    pub async fn onboard_connection(
        &self,
        input: OnboardingInput,
        models: Vec<String>,
    ) -> Result<OnboardingSaveResult, RequestFailure> {
        let target = input.target.clone();
        let mut value = onboarding_value(input);
        value["enabledModelIds"] = serde_json::json!(models);
        let value = self
            .request(Operation::ConnectionOnboardingSave, value)
            .await?;
        let invalid = |error: ProtocolError| {
            self.disconnect();
            RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
        };
        let result = maka_protocol::onboarding::decode_save_result(&value).map_err(invalid)?;
        if let OnboardingSaveResult::Saved { connection } = &result {
            let valid = match target {
                OnboardingTarget::Create {
                    provider_type,
                    slug,
                    ..
                } => {
                    connection.provider_type == provider_type
                        && slug.is_none_or(|slug| slug == connection.slug)
                }
                OnboardingTarget::Existing { connection_id } => {
                    connection.connection_id == connection_id
                }
            };
            if !valid {
                return Err(invalid(ProtocolError::invalid(
                    "Onboarded connection does not match request",
                )));
            }
        }
        Ok(result)
    }
    pub async fn connection_catalog(
        &self,
        input: ConnectionCatalogQueryInput,
    ) -> Result<Value, RequestFailure> {
        let value = self
            .request(
                Operation::ConnectionCatalogQuery,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        let invalid = |error: ProtocolError| {
            self.disconnect();
            RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
        };
        let page = maka_protocol::configuration_pages::decode_catalog_query_result(&value)
            .map_err(invalid)?;
        let valid = match (&input, page["kind"].as_str()) {
            (ConnectionCatalogQueryInput::Start, Some("page")) => {
                page["items"].as_array().is_some_and(|items| {
                    items.is_empty()
                        || starts_at(
                            &items[0],
                            &ConnectionCatalogCursor::Connection {
                                connection_index: 0,
                            },
                        )
                })
            }
            (ConnectionCatalogQueryInput::Continue { revision, cursor }, Some("page")) => {
                page["revision"] == *revision
                    && page["items"]
                        .as_array()
                        .and_then(|items| items.first())
                        .is_some_and(|item| starts_at(item, cursor))
            }
            (ConnectionCatalogQueryInput::Continue { revision, .. }, Some("revision_changed")) => {
                page["expectedRevision"] == *revision && page["actualRevision"] != *revision
            }
            _ => false,
        };
        if !valid {
            return Err(invalid(ProtocolError::invalid(
                "Connection catalog result does not match request",
            )));
        }
        Ok(page)
    }
}

fn onboarding_value(input: OnboardingInput) -> Value {
    serde_json::json!({"target":input.target,"apiKey":input.api_key,"baseUrl":input.base_url})
}

fn starts_at(item: &Value, cursor: &ConnectionCatalogCursor) -> bool {
    let cursor = serde_json::to_value(cursor).expect("wire cursor");
    item["kind"] == cursor["part"]
        && item["connectionIndex"] == cursor["connectionIndex"]
        && item.get("itemIndex") == cursor.get("itemIndex")
}
