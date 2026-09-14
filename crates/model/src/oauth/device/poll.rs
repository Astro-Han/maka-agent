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
impl DeviceAuthorization {
    /// Cancellation is observed only before a grant request or after a pending
    /// reply. The Host must retain this future through admitted requests and
    /// retain successful tokens even if cancellation arrived meanwhile. Copilot
    /// tokens are returned only after its model entitlement has been verified.
    pub async fn finish(
        self,
        cancel: &CancellationToken,
        mut boundary: impl FnMut(PollBoundary),
    ) -> Result<Tokens> {
        let client = &self.client;
        let mut interval = self.interval;
        let mut sleep_first = self.provider != Provider::OpenaiCodex;
        loop {
            let remaining = self
                .expires_at
                .checked_sub(now()?)
                .filter(|v| *v > 0)
                .ok_or(ErrorKind::Expired)?;
            if sleep_first {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(ErrorKind::Aborted.into()),
                    _ = tokio::time::sleep(Duration::from_millis(interval.min(remaining))) => {}
                }
            }
            if cancel.is_cancelled() {
                return Err(ErrorKind::Aborted.into());
            }
            if now()? >= self.expires_at {
                return Err(ErrorKind::Expired.into());
            }
            boundary(PollBoundary::Admitted);
            let request = if self.provider == Provider::OpenaiCodex {
                client
                    .http
                    .post("https://auth.openai.com/api/accounts/deviceauth/token")
                    .header("content-type", "application/json")
                    .body(
                        json!({"device_auth_id":self.code,"user_code":self.user_code}).to_string(),
                    )
            } else {
                client.form(self.provider).form(&[
                    ("client_id", client_id(self.provider)),
                    ("device_code", &self.code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
            };
            let response = client.request(request, None).await?;
            let retry = match self.provider {
                Provider::OpenaiCodex if response.ok() => {
                    boundary(PollBoundary::Exchanging);
                    let code = text(&response.payload["authorization_code"], 32 * 1024)?;
                    let verifier = text(&response.payload["code_verifier"], 32 * 1024)?;
                    let response = client
                        .request(
                            client
                                .form(self.provider)
                                .header("user-agent", "maka-desktop/0.1.0 (oauth-subscription)")
                                .form(&[
                                    ("grant_type", "authorization_code"),
                                    ("client_id", client_id(self.provider)),
                                    ("code", &code),
                                    ("code_verifier", &verifier),
                                    (
                                        "redirect_uri",
                                        "https://auth.openai.com/deviceauth/callback",
                                    ),
                                ]),
                            None,
                        )
                        .await?;
                    if !response.ok() {
                        return Err(response.rejected());
                    }
                    return Tokens::decode(self.provider, &response.payload);
                }
                Provider::OpenaiCodex => matches!(response.status, 403 | 404),
                Provider::GithubCopilot if !response.ok() => false,
                Provider::XaiOauth if response.ok() => {
                    return Tokens::decode(self.provider, &response.payload);
                }
                _ => match response.code().as_deref() {
                    Some("authorization_pending") => true,
                    Some("slow_down") => {
                        interval = if self.provider == Provider::GithubCopilot {
                            response
                                .payload
                                .get("interval")
                                .map(|v| positive(v, 300).map(|v| v * 1000))
                                .transpose()?
                                .unwrap_or((interval + 5000).min(300_000))
                        } else {
                            (interval + 5000).min(300_000)
                        };
                        true
                    }
                    Some("expired_token") => return Err(ErrorKind::Expired.into()),
                    Some("access_denied") | Some("authorization_denied") => {
                        return Err(response.failure(ErrorKind::InvalidGrant));
                    }
                    None if self.provider == Provider::GithubCopilot => {
                        let tokens = Tokens::decode(self.provider, &response.payload)?;
                        client.verify_copilot(&tokens).await?;
                        return Ok(tokens);
                    }
                    _ => false,
                },
            };
            if !retry {
                return Err(response.failure(ErrorKind::ProviderRejected));
            }
            boundary(PollBoundary::Retry);
            if cancel.is_cancelled() {
                return Err(ErrorKind::Aborted.into());
            }
            sleep_first = true;
        }
    }
}
