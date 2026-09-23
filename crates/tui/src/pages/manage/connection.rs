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

use super::{Command, Entity, Kind, Target, Ticket};
use crate::{
    app::{Action, App},
    pages::connections::Row,
};
use maka_client::{Client, ClientError, RequestFailure};
use maka_protocol::configuration::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    EnabledModels,
    ModelOverrides,
    FetchModels,
    Test,
    Endpoint,
    Enable,
    Disable,
    Remove,
}
impl Change {
    pub fn label(self) -> &'static str {
        match self {
            Self::EnabledModels => "connection-enabled-models",
            Self::ModelOverrides => "connection-model-overrides",
            Self::FetchModels => "connection-models-fetch",
            Self::Test => "connection-test",
            Self::Endpoint => "connection-endpoint",
            Self::Enable => "connection-enable",
            Self::Disable => "connection-disable",
            Self::Remove => "connection-remove",
        }
    }
    pub fn note(self) -> &'static str {
        match self {
            Self::EnabledModels => "enabled-model-default-note",
            Self::ModelOverrides => "model-profile-note",
            Self::FetchModels => "connection-models-fetch-note",
            Self::Test => "connection-test-note",
            Self::Endpoint => "connection-endpoint-note",
            Self::Enable => "connection-enable-note",
            Self::Disable => "connection-disable-note",
            Self::Remove => "connection-remove-note",
        }
    }
}

pub(super) fn update(row: &Row, kind: Kind, name: &str) -> UpdateCatalogConnectionInput {
    UpdateCatalogConnectionInput {
        expected: basis(row),
        changes: ConnectionCatalogEntryUpdate {
            name: if kind == Kind::Rename {
                name.into()
            } else {
                row.name.clone()
            },
            base_url: if kind == Kind::Connection(Change::Endpoint) {
                // The review step has already canonicalized and validated this value.
                validation::normalize_base_url(Some(name), Some(&row.provider))
                    .expect("reviewed endpoint")
            } else {
                row.base_url.clone()
            },
            enabled: match kind {
                Kind::Connection(Change::Enable) => true,
                Kind::Connection(Change::Disable) => false,
                _ => row.enabled,
            },
            enabled_model_ids: row.model_ids.clone(),
            model_overrides: Patch::Keep,
            request_body_overlay: Patch::Keep,
        },
    }
}
fn basis(row: &Row) -> ConnectionVersionBasis {
    ConnectionVersionBasis {
        connection_id: row.id.clone(),
        revision: row.revision,
    }
}

pub(super) async fn execute(
    client: &Client,
    ticket: &Ticket,
) -> Result<CatalogMutationResult, RequestFailure> {
    let Entity::Connection(row) = &ticket.target.entity else {
        return Err(RequestFailure::NotDispatched(ClientError::Protocol(
            "Invalid connection target".into(),
        )));
    };
    match ticket.kind {
        Kind::Connection(Change::Remove) => {
            client
                .remove_connection(RemoveCatalogConnectionInput {
                    expected: basis(row),
                })
                .await
        }
        Kind::Rename
        | Kind::Connection(
            Change::Endpoint
            | Change::Enable
            | Change::Disable
            | Change::EnabledModels
            | Change::ModelOverrides,
        ) => {
            let mut input = update(row, ticket.kind, &ticket.text);
            if let Some(ids) = &ticket.enabled_model_ids {
                input.changes.enabled_model_ids = ids.clone();
            }
            if let Some(profiles) = &ticket.model_overrides {
                input.changes.model_overrides = Patch::Set(profiles.clone());
            }
            client.update_connection(input).await
        }
        _ => Err(RequestFailure::NotDispatched(ClientError::Protocol(
            "Invalid connection change".into(),
        ))),
    }
}

impl App {
    pub(super) fn connection_management_commands(
        &self,
        root: &str,
        epoch: &str,
    ) -> Vec<(Action, &'static str)> {
        let Some(row) = self
            .connections
            .rows
            .iter()
            .find(|row| Some(&row.id) == self.connections.selected.as_ref())
        else {
            return vec![];
        };
        let target = Target {
            root: root.into(),
            epoch: epoch.into(),
            name: row.name.clone(),
            entity: Entity::Connection(row.clone()),
        };
        let mut kinds = vec![];
        if row.provider != "gemini-cli" {
            // The Host retires this provider: removal only.
            kinds.push(Kind::Rename);
            kinds.push(Kind::Connection(Change::EnabledModels));
            kinds.push(Kind::Connection(Change::ModelOverrides));
            if row.enabled {
                kinds.push(Kind::Connection(Change::FetchModels));
                kinds.push(Kind::Connection(Change::Test));
            }
            if validation::provider_auth_kind(&row.provider) == Ok(ProviderAuthKind::ApiKey) {
                kinds.push(Kind::Credential(super::credentials::Change::Set));
                kinds.push(Kind::Credential(super::credentials::Change::Clear));
            }
            if validation::provider_auth_kind(&row.provider)
                .is_ok_and(|auth| auth != ProviderAuthKind::OauthToken)
            {
                kinds.push(Kind::Connection(Change::Endpoint));
            }
            kinds.push(Kind::Connection(if row.enabled {
                Change::Disable
            } else {
                Change::Enable
            }));
        }
        kinds.push(Kind::Connection(Change::Remove));
        kinds
            .into_iter()
            .map(|kind| {
                (
                    Action::Manage(Command::Open(target.clone(), kind)),
                    kind.label(&target),
                )
            })
            .collect()
    }
    pub(super) fn connection_target_known(&self, target: &Target) -> bool {
        let Entity::Connection(row) = &target.entity else {
            return true;
        };
        self.connections
            .rows
            .iter()
            .any(|current| current.id == row.id && current.revision == row.revision)
    }
    pub fn rename_connection_action(&self) -> Option<Action> {
        self.management_commands()
            .into_iter()
            .map(|(action, _)| action)
            .find(|action| matches!(action, Action::Manage(Command::Open(_, Kind::Rename))))
    }
    pub(super) fn connection_acknowledged(
        &mut self,
        ticket: &Ticket,
        result: &CatalogMutationResult,
    ) {
        let Entity::Connection(basis) = &ticket.target.entity else {
            return;
        };
        let CatalogMutationResult::Committed { connection, .. } = result else {
            return;
        };
        if let Some(committed) = connection {
            if let Some(row) = self
                .connections
                .rows
                .iter_mut()
                .find(|row| row.id == basis.id)
            {
                // Apply only this acknowledged change. A newer catalog projection wins.
                if row.revision < committed.revision {
                    let mut changed = (**basis).clone();
                    let changes = update(&changed, ticket.kind, &ticket.text).changes;
                    changed.name = changes.name;
                    changed.base_url = changes.base_url;
                    changed.enabled = changes.enabled;
                    if let Some(ids) = &ticket.enabled_model_ids {
                        changed.model_ids = ids.clone();
                        changed.enabled_models = ids.len() as u64;
                        changed.default_model = changed.default_model.filter(|id| ids.contains(id));
                    }
                    changed.revision = committed.revision;
                    if !changed.enabled {
                        changed.default_model = None;
                    }
                    *row = std::sync::Arc::new(changed);
                }
            }
        } else {
            self.connections.rows.retain(|row| row.id != basis.id);
            if self.connections.selected.as_deref() == Some(&basis.id) {
                self.connections.selected = None;
            }
        }
    }
}

pub(super) fn endpoint(row: &Row, text: &str) -> Result<String, &'static str> {
    let normalized = validation::normalize_base_url(Some(text), Some(&row.provider))
        .map_err(|_| "connection-endpoint-invalid")?;
    let effective = normalized.as_deref().unwrap_or(
        validation::provider_default_base_url(&row.provider)
            .map_err(|_| "connection-endpoint-invalid")?,
    );
    if effective.is_empty() {
        return Err("connection-endpoint-required");
    }
    if normalized == row.base_url {
        return Err("connection-endpoint-unchanged");
    }
    Ok(effective.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Locale, LocalePreference, app::ConnectionState, i18n::I18n, navigation::Route};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;
    const ID: &str = "b746eb13-287c-4f3a-8590-dac93c0a1253";

    fn load(app: &mut App) {
        app.connections.refresh();
        app.connections.query().unwrap();
        app.connections.complete(Ok(json!({"kind":"page","revision":10,"connectionCount":1,
            "defaultTarget":null,"nextCursor":null,"items":[
            {"kind":"connection","connectionIndex":0,"connectionId":ID,"revision":7,"slug":"fixture","name":"Fixture",
                "providerType":"openai-compatible","enabled":true,"enabledModelIdCount":1,"baseUrl":"http://127.0.0.1/v1"},
            {"kind":"enabled_model_id","connectionIndex":0,"itemIndex":0,"modelId":"model"}
        ]})));
    }

    #[test]
    fn endpoint_review_shows_the_complete_destination_and_never_replays_uncertain_changes() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Visit(Route::Connections));
        load(&mut app);
        let Action::Manage(Command::Open(target, _)) = app.rename_connection_action().unwrap()
        else {
            panic!()
        };
        let Entity::Connection(row) = &target.entity else {
            panic!()
        };
        for invalid in [
            "file:///tmp/model",
            "https://user:secret@example.org/v1",
            "https://example.org/v1?key=value",
            "https://example.org/#fragment",
        ] {
            assert_eq!(endpoint(row, invalid), Err("connection-endpoint-invalid"));
        }
        assert_eq!(endpoint(row, ""), Err("connection-endpoint-required"));
        assert_eq!(
            endpoint(row, "http://127.0.0.1/v1"),
            Err("connection-endpoint-unchanged")
        );
        let mut provider = (**row).clone();
        provider.provider = "openai".into();
        assert_eq!(
            endpoint(&provider, " ").unwrap(),
            "https://api.openai.com/v1"
        );
        assert!(
            update(
                &provider,
                Kind::Connection(Change::Endpoint),
                "https://api.openai.com/v1"
            )
            .changes
            .base_url
            .is_none()
        );
        provider.provider = "openai-codex".into();
        app.connections.rows = vec![std::sync::Arc::new(provider)];
        assert!(
            !app.management_commands()
                .iter()
                .any(|(_, key)| *key == "connection-endpoint")
        );
        load(&mut app);
        app.apply(Action::Manage(Command::Open(
            target,
            Kind::Connection(Change::Endpoint),
        )));
        let mut screen = Terminal::new(TestBackend::new(80, 24)).unwrap();
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        app.input(Event::Paste("https://new.example.org/v1".into()));
        assert!(
            app.management_request().is_none(),
            "editing does not dispatch"
        );
        app.apply(Action::Manage(Command::Save));
        assert!(app.management.dialog.as_ref().unwrap().reviewing);
        let canonical = "https://new.example.org/v1";
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            for (width, height) in [(80, 24), (45, 24), (25, 10)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|f| {
                        super::super::draw(f, &mut app, f.area(), ratatui::style::Style::default())
                    })
                    .unwrap();
                assert_eq!(app.management_enabled(&Command::Save), width >= 45);
                if width >= 45 {
                    let compact = |s: &str| {
                        s.chars()
                            .filter(|c| !c.is_whitespace() && !"│─╭╮╰╯".contains(*c))
                            .collect::<String>()
                    };
                    let text: String = terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .map(|c| c.symbol())
                        .collect();
                    assert!(compact(&text).contains(canonical));
                    assert!(
                        compact(&text).contains(&compact(&app.i18n.text(Change::Endpoint.note())))
                    );
                }
                assert!(app.i18n.diagnostics().is_empty());
            }
        }
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        app.input(Event::Paste("https://evil.example".into()));
        assert_eq!(
            app.management.dialog.as_ref().unwrap().editor.text(),
            canonical,
            "review is read-only"
        );
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert!(app.management.dialog.is_none(), "review defaults to cancel");
        app.apply(
            app.management_commands()
                .into_iter()
                .find(|(_, key)| *key == "connection-endpoint")
                .unwrap()
                .0,
        );
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        app.input(Event::Paste(canonical.into()));
        app.apply(Action::Manage(Command::Save));
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let ticket = app.management_request().unwrap();
        assert_eq!(ticket.text, canonical);
        app.management_completed(ticket, Err(RequestFailure::Unknown(ClientError::Timeout)));
        assert!(!app.management_enabled(&Command::Edit));
        assert!(!app.management_enabled(&Command::Save));
        app.input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(app.management.dialog.is_none());
        assert!(
            app.management_commands().is_empty(),
            "new authoritative read required after unknown write"
        );
    }

    #[test]
    fn connection_management_preserves_configuration_confirms_impact_and_blocks_uncertain_writes() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Visit(Route::Connections));
        load(&mut app);
        let Action::Manage(Command::Open(target, _)) = app.rename_connection_action().unwrap()
        else {
            panic!()
        };
        let commands = app.management_commands();
        app.connections.refresh();
        assert_eq!(
            app.management_commands(),
            commands,
            "refreshing must not silently remove frozen menu entries"
        );
        assert!(
            app.management_enabled(&Command::Open(target.clone(), Kind::Rename)),
            "a complete confirmed row can open during refresh; the Host still enforces CAS"
        );
        load(&mut app);
        for change in [
            Change::Enable,
            Change::Disable,
            Change::Remove,
            Change::FetchModels,
        ] {
            app.apply(Action::Manage(Command::Open(
                target.clone(),
                Kind::Connection(change),
            )));
            for locale in Locale::ALL {
                app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
                for (width, height) in [(80, 24), (45, 20), (30, 10)] {
                    let mut screen = Terminal::new(TestBackend::new(width, height)).unwrap();
                    screen
                        .draw(|f| {
                            super::super::draw(
                                f,
                                &mut app,
                                f.area(),
                                ratatui::style::Style::default(),
                            )
                        })
                        .unwrap();
                    if width >= 45 {
                        let compact = |s: &str| {
                            s.chars()
                                .filter(|c| !c.is_whitespace() && !"│─╭╮╰╯".contains(*c))
                                .collect::<String>()
                        };
                        let text: String = screen
                            .backend()
                            .buffer()
                            .content
                            .iter()
                            .map(|c| c.symbol())
                            .collect();
                        assert!(
                            compact(&text).contains(&compact(&app.i18n.text(change.note()))),
                            "full impact note before enabling confirmation: {text}"
                        );
                        assert!(app.management_enabled(&Command::Save));
                    } else {
                        assert!(!app.management_enabled(&Command::Save));
                    }
                    assert!(app.i18n.diagnostics().is_empty());
                }
            }
            let mut screen = Terminal::new(TestBackend::new(80, 24)).unwrap();
            screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
            app.input(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )));
            assert!(
                app.management.dialog.is_none(),
                "lifecycle confirmation defaults to cancel"
            );
        }
        let Entity::Connection(row) = &target.entity else {
            panic!()
        };
        let renamed = update(row, Kind::Rename, "New name");
        assert_eq!(renamed.expected.revision, 7);
        assert_eq!(
            renamed.changes.base_url.as_deref(),
            Some("http://127.0.0.1/v1")
        );
        assert_eq!(renamed.changes.enabled_model_ids, ["model"]);
        let wire = serde_json::to_value(renamed).unwrap();
        assert!(wire["changes"].get("modelOverrides").is_none());
        assert!(wire["changes"].get("requestBodyOverlay").is_none());
        assert!(
            !update(row, Kind::Connection(Change::Disable), "ignored")
                .changes
                .enabled
        );
        let mut screen = Terminal::new(TestBackend::new(80, 24)).unwrap();
        for failure in [
            RequestFailure::Unknown(ClientError::Timeout),
            RequestFailure::Rejected(ClientError::Rejected(maka_protocol::OperationError {
                code: maka_protocol::OperationErrorCode::CommitOutcomeUnknown,
                message: "opaque".into(),
            })),
        ] {
            app.apply(Action::Manage(Command::Open(target.clone(), Kind::Rename)));
            screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
            let ticket = app.management_request().unwrap();
            assert!(app.management_request().is_none());
            app.management_completed(ticket, Err(failure));
            assert!(!app.management_enabled(&Command::Save));
            app.apply(Action::Manage(Command::Close));
            assert!(!app.management_enabled(&Command::Open(target.clone(), Kind::Rename)));
            load(&mut app);
        }
        app.apply(Action::Manage(Command::Open(target.clone(), Kind::Rename)));
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let ticket = app.management_request().unwrap();
        app.management_completed(
            ticket,
            Ok(super::super::Updated::Catalog(
                CatalogMutationResult::ConnectionStale {
                    expected: basis(row),
                    actual: Some(ConnectionVersionBasis {
                        connection_id: ID.into(),
                        revision: 8,
                    }),
                },
            )),
        );
        assert!(!app.management_enabled(&Command::Save));
        assert_eq!(
            app.management.dialog.as_ref().unwrap().error,
            Some("connection-edit-conflict")
        );
        app.apply(Action::Manage(Command::Close));
        load(&mut app);
        app.apply(Action::Manage(Command::Open(
            target.clone(),
            Kind::Connection(Change::Remove),
        )));
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let ticket = app.management_request().unwrap();
        app.apply(Action::Manage(Command::Close));
        app.apply(Action::Visit(Route::Settings));
        app.management_completed(
            ticket,
            Ok(super::super::Updated::Catalog(
                CatalogMutationResult::Committed {
                    catalog_revision: 11,
                    connection: None,
                },
            )),
        );
        assert_eq!(
            app.navigation.current(),
            Route::Settings,
            "late acknowledgement never changes page"
        );
        assert!(app.management.dialog.is_none());
    }

    #[test]
    fn model_fetch_rechecks_authority_without_guessing_enabled_ids_or_replaying_unknown_writes() {
        use super::super::Updated;
        use maka_protocol::connection_effects::*;
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Visit(Route::Connections));
        load(&mut app);
        let Action::Manage(Command::Open(target, _)) = app.rename_connection_action().unwrap()
        else {
            panic!()
        };
        let kind = Kind::Connection(Change::FetchModels);
        let mut screen = Terminal::new(TestBackend::new(80, 24)).unwrap();
        for (result, blocked, error) in [
            (
                ConnectionModelFetchResult::Failed {
                    error_class: ConnectionEffectFailureClass::Auth,
                },
                false,
                "connection-models-fetch-auth",
            ),
            (
                ConnectionModelFetchResult::Failed {
                    error_class: ConnectionEffectFailureClass::InvalidResponse,
                },
                false,
                "connection-models-fetch-invalid",
            ),
            (
                ConnectionModelFetchResult::Rejected {
                    reason: ConnectionEffectRejectionReason::CredentialNotConfigured,
                },
                true,
                "connection-models-fetch-key",
            ),
            (
                ConnectionModelFetchResult::Superseded {
                    changed: vec![ConnectionEffectChangedDomain::Credential],
                },
                true,
                "connection-models-fetch-changed",
            ),
        ] {
            app.apply(Action::Manage(Command::Open(target.clone(), kind)));
            screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
            let ticket = app.management_request().unwrap();
            assert!(app.management_request().is_none());
            app.management_completed(ticket, Ok(Updated::ModelFetch(result)));
            let dialog = app.management.dialog.as_ref().unwrap();
            assert_eq!(dialog.blocked, blocked);
            assert_eq!(dialog.error, Some(error));
            assert_eq!(
                dialog.focus, 0,
                "failure returns focus to Cancel, not network replay"
            );
            app.apply(Action::Manage(Command::Close));
        }
        app.apply(Action::Manage(Command::Open(target.clone(), kind)));
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let ticket = app.management_request().unwrap();
        app.management_completed(
            ticket,
            Err(RequestFailure::Rejected(ClientError::Rejected(
                maka_protocol::OperationError {
                    code: maka_protocol::OperationErrorCode::OperationUnavailable,
                    message: "network configuration or OAuth detail must not be rendered".into(),
                },
            ))),
        );
        assert_eq!(
            app.management.dialog.as_ref().unwrap().error,
            Some("connection-models-fetch-not-ready")
        );
        assert!(!app.management_enabled(&Command::Save));
        app.apply(Action::Manage(Command::Close));
        for current_revision in [7, 9] {
            load(&mut app);
            app.apply(Action::Manage(Command::Open(target.clone(), kind)));
            screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
            let ticket = app.management_request().unwrap();
            std::sync::Arc::make_mut(&mut app.connections.rows[0]).revision = current_revision;
            app.apply(Action::Manage(Command::Close));
            app.apply(Action::Visit(Route::Settings));
            app.management_completed(
                ticket,
                Ok(Updated::ModelFetch(ConnectionModelFetchResult::Committed {
                    catalog_revision: 12,
                    connection: ConnectionVersionBasis {
                        connection_id: ID.into(),
                        revision: 8,
                    },
                    model_count: 5,
                    source: ModelDiscoverySource::Fetched,
                    fetched_at: 1,
                })),
            );
            assert_eq!(app.navigation.current(), Route::Settings);
            assert!(app.management.dialog.is_none());
            assert_eq!(
                app.connections.rows.len(),
                usize::from(current_revision > 8),
                "old basis removed, newer projection preserved"
            );
        }
        app.apply(Action::Visit(Route::Connections));
        load(&mut app);
        app.apply(Action::Manage(Command::Open(target.clone(), kind)));
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let ticket = app.management_request().unwrap();
        app.management_completed(ticket, Err(RequestFailure::Unknown(ClientError::Timeout)));
        assert!(!app.management_enabled(&Command::Save));
        app.apply(Action::Manage(Command::Close));
        assert!(!app.management_enabled(&Command::Open(target, kind)));
    }
}
