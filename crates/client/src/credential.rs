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
use maka_protocol::{Operation, configuration::*};
use serde_json::json;

impl Client {
    pub async fn credential_status(
        &self,
        locator: CredentialLocator,
    ) -> Result<CredentialVaultQueryResult, RequestFailure> {
        let value = self
            .request(Operation::CredentialVaultQuery, json!({"locator":locator}))
            .await?;
        let result =
            decode_credential_query_result(&value).map_err(|_| self.credential_invalid())?;
        if matches!(&result,CredentialVaultQueryResult::Status{status} if status.locator!=locator)
            || matches!(&result, CredentialVaultQueryResult::ConnectionNotFound)
                && !matches!(locator, CredentialLocator::Connection { .. })
        {
            return Err(self.credential_invalid());
        }
        Ok(result)
    }
    pub async fn set_credential(
        &self,
        input: SetCredentialInput,
    ) -> Result<CredentialMutationResult, RequestFailure> {
        let expected = input.expected.as_ref().map(|basis| CredentialVersionBasis {
            locator: input.locator.clone(),
            credential_id: basis.credential_id.clone(),
            revision: basis.revision,
        });
        let mut wire =
            json!({"locator":input.locator,"expected":input.expected,"secret":input.secret});
        if let Some(connection) = &input.expected_connection {
            wire["expectedConnection"] = json!(connection);
        }
        let value = self.request(Operation::CredentialVaultSet, wire).await?;
        self.credential_result(
            Operation::CredentialVaultSet,
            value,
            &input.locator,
            expected.as_ref(),
            input.expected_connection.as_ref(),
        )
    }
    pub async fn delete_credential(
        &self,
        input: DeleteCredentialInput,
    ) -> Result<CredentialMutationResult, RequestFailure> {
        let value = self
            .request(
                Operation::CredentialVaultDelete,
                serde_json::to_value(&input).expect("wire input"),
            )
            .await?;
        self.credential_result(
            Operation::CredentialVaultDelete,
            value,
            &input.expected.locator,
            Some(&input.expected),
            None,
        )
    }
    fn credential_invalid(&self) -> RequestFailure {
        self.disconnect();
        RequestFailure::Unknown(ClientError::Protocol(
            "Credential response does not match request".into(),
        ))
    }
    fn credential_result(
        &self,
        operation: Operation,
        value: serde_json::Value,
        locator: &CredentialLocator,
        expected: Option<&CredentialVersionBasis>,
        connection: Option<&ConnectionCredentialTarget>,
    ) -> Result<CredentialMutationResult, RequestFailure> {
        let result = decode_credential_mutation_result(operation, &value)
            .map_err(|_| self.credential_invalid())?;
        let valid = match &result {
            CredentialMutationResult::Committed { status, .. } if &status.locator == locator => {
                match (&status.state, operation) {
                    (CredentialState::Absent, Operation::CredentialVaultDelete) => true,
                    (
                        CredentialState::Configured {
                            credential_id,
                            revision,
                            ..
                        },
                        Operation::CredentialVaultSet,
                    ) => {
                        if matches!(
                            locator,
                            CredentialLocator::Connection {
                                kind: ConnectionCredentialKind::OauthToken,
                                ..
                            }
                        ) {
                            *revision == 1
                                && expected
                                    .is_none_or(|basis| basis.credential_id != *credential_id)
                        } else {
                            expected.map_or(*revision == 1, |basis| {
                                basis.credential_id == *credential_id
                                    && basis.revision + 1 == *revision
                            })
                        }
                    }
                    _ => false,
                }
            }
            CredentialMutationResult::CredentialStale {
                expected: returned,
                actual,
            } => {
                returned.as_ref() == expected
                    && actual.as_ref() != expected
                    && actual
                        .as_ref()
                        .is_none_or(|basis| &basis.locator == locator)
            }
            CredentialMutationResult::ConnectionStale {
                expected: returned,
                actual,
            } => connection.is_some_and(|basis| {
                returned.connection_id == basis.connection_id
                    && returned.revision == basis.revision
                    && actual.as_ref().is_none_or(|current| {
                        current.connection_id == basis.connection_id
                            && current.revision != basis.revision
                    })
            }),
            CredentialMutationResult::ConnectionNotFound => {
                matches!(locator, CredentialLocator::Connection { .. })
            }
            _ => false,
        };
        if !valid {
            return Err(self.credential_invalid());
        }
        Ok(result)
    }
}
