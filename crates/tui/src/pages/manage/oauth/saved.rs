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

use super::{
    App, ConnectionIdentity, ConnectionState, LoginStart, LoginTarget, PROVIDERS, Provider,
    Request, State,
};
use serde::{Deserialize, Serialize};

/// Recovery identity only. Never persist presentation URLs, codes, capabilities
/// or phases whose historical value could be mistaken for the current outcome.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    provider: Provider,
    start: LoginStart,
    connection: Option<ConnectionIdentity>,
}

impl Checkpoint {
    pub fn validate(&self) -> Result<(), String> {
        let valid = || -> Result<(), maka_protocol::ProtocolError> {
            maka_protocol::oauth::decode_start(&serde_json::json!(self.start))?;
            if let Some(connection) = &self.connection {
                // The original projection decoder validates the identity shape;
                // this temporary phase is never installed as a live Host result.
                let projection = maka_protocol::oauth::decode_login(&serde_json::json!({
                    "attemptId": self.start.attempt_id,
                    "connection": connection, "phase": "awaiting_authorization"
                }))?;
                maka_protocol::oauth::assert_start(&self.start, &projection)?;
            }
            Ok(())
        };
        if valid().is_err()
            || matches!(&self.start.target, LoginTarget::Create {provider_type, ..} if *provider_type != self.provider)
            || self
                .connection
                .as_ref()
                .is_some_and(|connection| connection.provider_type != self.provider)
        {
            return Err("Invalid saved OAuth identity".into());
        }
        Ok(())
    }
}

impl State {
    pub fn checkpoint(&self) -> Option<Checkpoint> {
        self.attempt
            .as_ref()
            .filter(|_| !self.terminal())
            .map(|start| Checkpoint {
                provider: PROVIDERS[self.provider],
                start: start.clone(),
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
        self.provider = PROVIDERS
            .iter()
            .position(|provider| *provider == saved.provider)
            .expect("known provider");
        if let LoginTarget::Existing { connection_id } = &saved.start.target {
            self.existing = Some(connection_id.clone());
            self.connection_label = saved.connection.as_ref().map_or_else(
                || connection_id.clone(),
                |connection| connection.slug.clone(),
            );
        } else if let LoginTarget::Create { name, slug, .. } = &saved.start.target {
            self.connection_label = name
                .iter()
                .chain(slug.iter())
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" · ");
        }
        self.attempt = Some(saved.start);
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
    use super::*;
    use crate::i18n::{I18n, Locale, LocalePreference};
    use serde_json::json;

    #[test]
    fn recovery_keeps_only_valid_identity_and_start_waits_for_exact_durable_checkpoint() {
        let basis = json!({"provider":"openai-codex", "start":{
            "attemptId":"original-attempt", "target":{"kind":"create", "providerType":"openai-codex", "slug":"work", "name":"Work"}},
            "connection":{"connectionId":"account", "providerType":"openai-codex", "slug":"work"}});
        let saved: Checkpoint = serde_json::from_value(basis.clone()).unwrap();
        saved.validate().unwrap();
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Auto, Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.management.oauth.restore("root", saved);
        assert!(app.management.dialog.is_none());
        assert!(app.management.oauth.projection.is_none());
        assert!(
            app.oauth_request().is_none(),
            "reopening must not perform a login or poll"
        );
        assert_eq!(
            serde_json::to_value(app.management.oauth.checkpoint()).unwrap(),
            basis
        );
        let state = &mut app.management.oauth;
        state.display = Some((
            "https://secret.example/device".into(),
            Some("SECRET-CODE".into()),
        ));
        assert_eq!(
            serde_json::to_value(state.checkpoint()).unwrap(),
            basis,
            "presentation never persists"
        );
        state.requested = Some(super::super::Operation::Query);
        let query = app.oauth_request().unwrap();
        assert!(!query.needs_checkpoint());
        assert_eq!(query.connection.as_ref().unwrap().connection_id, "account");
        assert!(!app.oauth_after_checkpoint(&query, &Ok(())));
        let state = &mut app.management.oauth;
        state.pending = None;
        state.requested = Some(super::super::Operation::Start);
        let start = app.oauth_request().unwrap();
        assert!(start.needs_checkpoint());
        let mut stale = start.clone();
        stale.sequence -= 1;
        assert!(!app.oauth_after_checkpoint(&stale, &Ok(())));
        assert!(app.oauth_after_checkpoint(&start, &Ok(())));
        assert!(
            !app.oauth_after_checkpoint(&start, &Ok(())),
            "one completion cannot dispatch twice"
        );
        for outcome in ["failed", "epoch", "quit"] {
            app.management.oauth.pending = None;
            app.management.oauth.requested = Some(super::super::Operation::Start);
            let request = app.oauth_request().unwrap();
            let result = match outcome {
                "failed" => Err("storage unavailable".into()),
                "epoch" => {
                    app.connection = ConnectionState::Connected {
                        root_id: "root".into(),
                        epoch: "next-epoch".into(),
                    };
                    Ok(())
                }
                _ => {
                    app.oauth_abandon_checkpoint();
                    Ok(())
                }
            };
            assert!(!app.oauth_after_checkpoint(&request, &result));
            assert!(app.management.oauth.attempt.is_some());
            assert!(app.oauth_request().is_none());
        }
        for (pointer, value) in [
            ("/start/attemptId", json!("bad\nidentity")),
            ("/provider", json!("xai-oauth")),
            ("/connection/slug", json!("different")),
            ("/connection/connectionId", json!("bad identity")),
            ("/connection/providerType", json!("github-copilot")),
        ] {
            let mut invalid = basis.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(
                serde_json::from_value::<Checkpoint>(invalid)
                    .unwrap()
                    .validate()
                    .is_err(),
                "{pointer}"
            );
        }
        let mut existing = basis.clone();
        existing["start"]["target"] = json!({"kind":"existing", "connectionId":"other"});
        assert!(
            serde_json::from_value::<Checkpoint>(existing)
                .unwrap()
                .validate()
                .is_err()
        );
        let mut unknown = basis;
        unknown["url"] = json!("https://must-not-persist.example");
        assert!(serde_json::from_value::<Checkpoint>(unknown).is_err());
    }
}
