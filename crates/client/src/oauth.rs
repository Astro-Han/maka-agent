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
use maka_protocol::{Operation, oauth::*};
use serde_json::{Value, json};

impl Client {
    pub async fn oauth_enrollment(
        &self,
        provider: Provider,
    ) -> Result<EnrollmentProjection, RequestFailure> {
        let value = self
            .request(
                Operation::OauthEnrollmentQuery,
                json!({"provider":provider}),
            )
            .await?;
        let result = decode_enrollment_result(&value).map_err(|_| self.oauth_invalid())?;
        if result.provider != provider {
            return Err(self.oauth_invalid());
        }
        Ok(result)
    }

    /// Keep this input when the outcome is unknown: query the same attempt,
    /// never generate another attempt ID or automatically replay start.
    pub async fn start_oauth_login(
        &self,
        input: &LoginStart,
    ) -> Result<LoginProjection, RequestFailure> {
        let value = self
            .request(Operation::OauthLoginStart, json!(input))
            .await?;
        self.oauth_login_result(value, input, None)
    }

    /// `connection` is the identity from the first confirmed projection. Only
    /// an unknown start without a projection should use None. After reconnect,
    /// callers must also establish that they are addressing the original Root.
    pub async fn query_oauth_login(
        &self,
        input: &LoginStart,
        connection: Option<&ConnectionIdentity>,
    ) -> Result<LoginProjection, RequestFailure> {
        self.oauth_attempt(Operation::OauthLoginQuery, input, connection)
            .await
    }

    /// Cancellation is a request, not a terminal outcome. The Host may return
    /// an in-flight phase or an already committed authenticated projection.
    pub async fn cancel_oauth_login(
        &self,
        input: &LoginStart,
        connection: Option<&ConnectionIdentity>,
    ) -> Result<LoginProjection, RequestFailure> {
        self.oauth_attempt(Operation::OauthLoginCancel, input, connection)
            .await
    }

    async fn oauth_attempt(
        &self,
        operation: Operation,
        input: &LoginStart,
        connection: Option<&ConnectionIdentity>,
    ) -> Result<LoginProjection, RequestFailure> {
        // Query/cancel carry only an attempt ID on the wire. Validate the local
        // target too, so a malformed recovery basis causes no Host operation.
        decode_start(&json!(input)).map_err(|_| {
            RequestFailure::NotDispatched(ClientError::Protocol(
                "Invalid OAuth attempt basis".into(),
            ))
        })?;
        if connection.is_some_and(|identity| !input.target.matches(identity)) {
            return Err(RequestFailure::NotDispatched(ClientError::Protocol(
                "OAuth connection does not match attempt target".into(),
            )));
        }
        let value = self
            .request(operation, json!({"attemptId":input.attempt_id}))
            .await?;
        self.oauth_login_result(value, input, connection)
    }

    fn oauth_login_result(
        &self,
        value: Value,
        input: &LoginStart,
        connection: Option<&ConnectionIdentity>,
    ) -> Result<LoginProjection, RequestFailure> {
        let result = decode_login(&value).map_err(|_| self.oauth_invalid())?;
        assert_start(input, &result).map_err(|_| self.oauth_invalid())?;
        if connection.is_some_and(|identity| *identity != result.connection) {
            return Err(self.oauth_invalid());
        }
        Ok(result)
    }

    fn oauth_invalid(&self) -> RequestFailure {
        self.disconnect();
        RequestFailure::Unknown(ClientError::Protocol(
            "OAuth response does not match attempt or connection".into(),
        ))
    }
}
