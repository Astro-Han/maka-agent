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
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct State {
    pub credential: OAuthCredential,
    pub outcome: Outcome,
}

pub(super) enum Outcome {
    Ready,
    // Keep a spent replacement on every SQL failure. Retrying persistence is
    // safe; retrying the old remote grant to resolve uncertainty is not.
    Replacement(String),
    Failed(String),
}

impl State {
    pub async fn resolve(
        &mut self,
        provider: Provider,
        client: Client,
        shutdown: &CancellationToken,
        superseded: &CancellationToken,
    ) -> Result<ResolvedToken, ModelError> {
        if let Outcome::Failed(message) = &self.outcome {
            return Err(failure(message));
        }
        if matches!(self.outcome, Outcome::Replacement(_)) {
            self.settle().await?;
        }
        let current = self
            .credential
            .current_generation()
            .await
            .map_err(failure)?
            .ok_or_else(|| failure("OAuth credential was superseded"))?;
        if current.basis() != self.credential.basis() {
            return Err(failure("OAuth credential was superseded"));
        }
        let tokens = Tokens::from_stored(self.credential.secret()).map_err(failure)?;
        if tokens.expires_at.saturating_sub(now()?) > 5 * 60 * 1000 {
            return Ok(ResolvedToken {
                access_token: tokens.access_token,
                credential: self.credential.clone(),
            });
        }
        if shutdown.is_cancelled() || superseded.is_cancelled() {
            return Err(ModelError::Cancelled);
        }
        let tokens = match client.refresh(provider, tokens).await {
            Ok(tokens) => tokens,
            Err(error) => {
                // Even a transport error can follow a consumed rotating grant.
                // All waiters observe this failure until a new credential is supplied.
                self.outcome = Outcome::Failed(error.to_string());
                return Err(failure(error));
            }
        };
        self.outcome = Outcome::Replacement(serde_json::to_string(&tokens).map_err(failure)?);
        self.settle().await?;
        Ok(ResolvedToken {
            access_token: tokens.access_token,
            credential: self.credential.clone(),
        })
    }

    async fn settle(&mut self) -> Result<(), ModelError> {
        let Outcome::Replacement(replacement) = &self.outcome else {
            unreachable!("received replacement")
        };
        // The same check handles an earlier unknown commit, including a lost
        // acknowledgement. Never use a replacement without canonical proof.
        let current = self
            .credential
            .current_generation()
            .await
            .map_err(failure)?
            .ok_or_else(|| failure("OAuth credential was superseded"))?;
        let next = self
            .credential
            .basis()
            .revision
            .checked_add(1)
            .ok_or_else(|| failure("OAuth credential revision exhausted"))?;
        if current.basis().revision == next && current.secret() == replacement {
            self.credential = current;
            self.outcome = Outcome::Ready;
            return Ok(());
        }
        if current.basis() != self.credential.basis() {
            return Err(failure("OAuth credential was superseded"));
        }
        self.credential
            .commit_refresh(replacement.clone(), now()?)
            .await
            .map_err(failure)?
            .ok_or_else(|| failure("OAuth credential was superseded"))?;
        let current = self
            .credential
            .current_generation()
            .await
            .map_err(failure)?
            .ok_or_else(|| failure("OAuth credential was superseded"))?;
        if current.basis().revision != next || current.secret() != replacement {
            return Err(failure("OAuth credential was superseded"));
        }
        self.credential = current;
        self.outcome = Outcome::Ready;
        Ok(())
    }
}

fn now() -> Result<u64, ModelError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|time| u64::try_from(time.as_millis()).ok())
        .filter(|time| *time <= 9_007_199_254_740_991)
        .ok_or_else(|| failure("Invalid system time for OAuth"))
}
