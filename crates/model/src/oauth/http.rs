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

use super::*;
use reqwest::RequestBuilder;

pub(super) struct Response {
    pub status: u16,
    pub payload: Value,
}
impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    pub fn failure(&self, kind: ErrorKind) -> Error {
        Error {
            kind,
            status: Some(self.status),
        }
    }
    pub fn rejected(&self) -> Error {
        self.failure(match self.code().as_deref() {
            Some("invalid_grant") => ErrorKind::InvalidGrant,
            Some("invalid_token") => ErrorKind::InvalidToken,
            _ => ErrorKind::ProviderRejected,
        })
    }
    pub fn code(&self) -> Option<String> {
        let payload = self.payload.as_object()?;
        payload
            .get("error")
            .and_then(Value::as_str)
            .or_else(|| payload.get("error")?.get("code")?.as_str())
            .or_else(|| payload.get("error")?.get("type")?.as_str())
            .or_else(|| payload.get("code")?.as_str())
            .map(str::to_lowercase)
    }
}
impl Client {
    pub(super) async fn request(
        &self,
        request: RequestBuilder,
        cancel: Option<&CancellationToken>,
    ) -> Result<Response> {
        let perform = async {
            let mut response = request
                .send()
                .await
                .map_err(|_| Error::from(ErrorKind::OutcomeUnknown))?;
            let status = response.status().as_u16();
            let fail = |kind| Error {
                kind,
                status: Some(status),
            };
            if let Some(length) = response.headers().get("content-length") {
                let length = length.to_str().ok().and_then(|s| s.parse::<u64>().ok());
                if length.is_none_or(|n| n > 64 * 1024) {
                    return Err(fail(ErrorKind::ResponseTooLarge));
                }
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| fail(ErrorKind::OutcomeUnknown))?
            {
                if chunk.len() > 64 * 1024 - bytes.len() {
                    return Err(fail(ErrorKind::ResponseTooLarge));
                }
                bytes.extend_from_slice(&chunk);
            }
            let payload =
                serde_json::from_slice(&bytes).map_err(|_| fail(ErrorKind::InvalidResponse))?;
            Ok(Response { status, payload })
        };
        if let Some(cancel) = cancel {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(ErrorKind::Aborted.into()),
                result = perform => result,
            }
        } else {
            perform.await
        }
    }
    pub(super) fn form(&self, provider: Provider) -> RequestBuilder {
        self.http
            .post(token_endpoint(provider))
            .header("accept", "application/json")
    }
}
