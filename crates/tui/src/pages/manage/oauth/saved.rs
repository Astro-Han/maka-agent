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

use super::{App, ConnectionIdentity, ConnectionState, LoginRecovery, LoginTarget, Request, State};
use serde::{Deserialize, Serialize};

/// Public recovery identity only. Authentication input and presentation never persist.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    attempt: LoginRecovery,
    connection: Option<ConnectionIdentity>,
}

impl Checkpoint {
    pub fn validate(&self) -> Result<(), String> {
        self.attempt.validate()?;
        if let Some(connection) = &self.connection {
            let projection = maka_protocol::oauth::decode_login(&serde_json::json!({
                "attemptId": self.attempt.attempt_id,
                "connection": connection, "phase": "awaiting_authorization"
            }))
            .map_err(|error| error.to_string())?;
            maka_protocol::oauth::assert_recovery(&self.attempt, &projection)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

impl State {
    pub fn checkpoint(&self) -> Option<Checkpoint> {
        self.attempt
            .as_ref()
            .filter(|_| !self.terminal())
            .map(|attempt| Checkpoint {
                attempt: attempt.clone(),
                connection: self
                    .projection
                    .as_ref()
                    .map(|projection| projection.connection.clone())
                    .or_else(|| self.recovered_connection.clone()),
            })
    }

    pub fn restore(&mut self, root: &str, saved: Checkpoint) {
        *self = Self::default();
        self.root = root.into();
        self.connection_label = match &saved.attempt.target {
            LoginTarget::Existing { expected, .. } => {
                self.existing = Some(expected.clone());
                expected.slug.clone()
            }
            LoginTarget::Create { name, slug, .. } => format!("{name} · {slug}"),
        };
        self.attempt = Some(saved.attempt);
        self.recovered_connection = saved.connection;
        self.error = Some("oauth-unknown");
    }
}

impl App {
    /// Only the exact start included in a successful checkpoint may dispatch.
    /// Epoch changes and local cancellation invalidate a delayed IO completion.
    pub fn oauth_after_checkpoint(
        &mut self,
        request: &Request,
        result: &Result<(), String>,
    ) -> bool {
        let state = &mut self.management.oauth;
        if !state.awaiting_checkpoint || state.pending.as_ref() != Some(request) {
            return false;
        }
        state.awaiting_checkpoint = false;
        if result.is_ok()
            && !self.closing
            && matches!(&self.connection, ConnectionState::Connected {root_id, epoch} if *root_id == request.root && *epoch == request.epoch)
        {
            return true;
        }
        state.pending = None;
        state.error = Some(if result.is_err() {
            "oauth-save-failed"
        } else {
            "oauth-unknown"
        });
        false
    }

    pub fn oauth_abandon_checkpoint(&mut self) {
        let state = &mut self.management.oauth;
        if state.awaiting_checkpoint {
            state.awaiting_checkpoint = false;
            state.pending = None;
            state.error = Some("oauth-unknown");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Call, LoginStart, Operation};
    use super::*;
    use crate::{
        i18n::{I18n, Locale, LocalePreference},
        providers::fixtures::entry,
    };
    use serde_json::json;

    #[test]
    fn recovery_is_observation_only_and_dispatch_requires_its_exact_durable_checkpoint() {
        let provider = entry("external-login", false).identity;
        let start = LoginStart {
            attempt_id: "original-attempt".into(),
            target: LoginTarget::Create {
                provider: provider.clone(),
                configuration: json!({}),
                slug: "work".into(),
                name: "Work".into(),
            },
            authentication: maka_protocol::oauth::AuthenticationInput {
                method: "key".into(),
                input: json!({"apiKey":"PRIVATE"}),
            },
        };
        let connection = ConnectionIdentity {
            connection_id: "account".into(),
            provider,
            slug: "work".into(),
        };
        let saved = Checkpoint {
            attempt: start.recovery(),
            connection: Some(connection),
        };
        saved.validate().unwrap();
        let basis = serde_json::to_value(&saved).unwrap();
        assert!(!basis.to_string().contains("PRIVATE"));
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Auto, Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.management.oauth.restore("root", saved);
        assert!(
            app.oauth_request().is_none(),
            "restore does not automatically dispatch or poll"
        );
        assert_eq!(
            serde_json::to_value(app.management.oauth.checkpoint()).unwrap(),
            basis
        );
        app.management.oauth.requested = Some(Operation::Query);
        let query = app.oauth_request().unwrap();
        let Call::Query(observation) = &query.call else {
            panic!()
        };
        assert_eq!(observation.attempt, start.recovery());
        assert_eq!(
            observation.connection.as_ref().unwrap().connection_id,
            "account"
        );
        assert!(!query.needs_checkpoint());
        assert!(!app.oauth_after_checkpoint(&query, &Ok(())));
        app.management.oauth.pending = None;
        app.management.oauth.requested = Some(Operation::Start);
        assert!(
            app.oauth_request().is_none(),
            "recovery cannot recreate secret-bearing input"
        );

        for outcome in ["success", "failed", "epoch", "quit"] {
            app.connection = ConnectionState::Connected {
                root_id: "root".into(),
                epoch: "epoch".into(),
            };
            let state = &mut app.management.oauth;
            state.prepared = Some(start.clone());
            state.requested = Some(Operation::Start);
            let request = app.oauth_request().unwrap();
            assert!(request.needs_checkpoint());
            let mut stale = request.clone();
            stale.sequence = stale.sequence.wrapping_sub(1);
            assert!(!app.oauth_after_checkpoint(&stale, &Ok(())));
            let result = match outcome {
                "failed" => Err("storage unavailable".into()),
                "epoch" => {
                    app.connection = ConnectionState::Connected {
                        root_id: "root".into(),
                        epoch: "next".into(),
                    };
                    Ok(())
                }
                "quit" => {
                    app.oauth_abandon_checkpoint();
                    Ok(())
                }
                _ => Ok(()),
            };
            assert_eq!(
                app.oauth_after_checkpoint(&request, &result),
                outcome == "success"
            );
            assert!(
                !app.oauth_after_checkpoint(&request, &Ok(())),
                "checkpoint completion is consumed once"
            );
            app.management.oauth.pending = None;
        }
        for (pointer, value) in [
            ("/attempt/attemptId", json!("invalid\nidentity")),
            ("/connection/slug", json!("different")),
            ("/connection/provider/packageId", json!("other.models")),
        ] {
            let mut invalid = basis.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(
                serde_json::from_value::<Checkpoint>(invalid)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut secret = basis;
        secret["authentication"] = json!({"apiKey":"PRIVATE"});
        assert!(serde_json::from_value::<Checkpoint>(secret).is_err());
    }
}
