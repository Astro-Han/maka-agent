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

mod view;
use super::{Command, Entity, Kind, Target, Ticket, Updated};
use crate::{app::App, editor::Editor, pages::connections::Row};
use maka_client::{Client, ClientError, RequestFailure};
use maka_protocol::configuration::*;
pub(super) use view::draw;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Set,
    Clear,
}
impl Change {
    pub fn label(self) -> &'static str {
        match self {
            Self::Set => "credential-key",
            Self::Clear => "credential-clear",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    generation: u64,
    target: Target,
}
impl Request {
    pub fn locator(&self) -> CredentialLocator {
        let Entity::Connection(row) = &self.target.entity else {
            unreachable!()
        };
        locator(row)
    }
}
pub(super) struct State {
    pub generation: u64,
    pub status: Option<CredentialStatus>,
    pub requested: bool,
    pub failed: bool,
}
impl State {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            status: None,
            requested: true,
            failed: false,
        }
    }
}
pub(super) fn editor() -> Editor {
    Editor::bounded(10240, "credential-key-too-large")
}
fn locator(row: &Row) -> CredentialLocator {
    CredentialLocator::Connection {
        connection_id: row.id.clone(),
        kind: ConnectionCredentialKind::ApiKey,
    }
}
pub(super) fn address(row: &Row) -> Option<String> {
    let raw = row
        .base_url
        .as_deref()
        .or_else(|| validation::provider_default_base_url(&row.provider).ok())?;
    validation::normalize_base_url(Some(raw), None)
        .ok()
        .flatten()
}
fn basis(status: &CredentialStatus) -> Option<CredentialVersionBasis> {
    let CredentialState::Configured {
        credential_id,
        revision,
        ..
    } = &status.state
    else {
        return None;
    };
    Some(CredentialVersionBasis {
        locator: status.locator.clone(),
        credential_id: credential_id.clone(),
        revision: *revision,
    })
}
pub(super) async fn execute(
    client: &Client,
    ticket: &Ticket,
    secret: Option<String>,
) -> Result<Updated, RequestFailure> {
    let invalid =
        || RequestFailure::NotDispatched(ClientError::Protocol("Missing credential basis".into()));
    let Entity::Connection(row) = &ticket.target.entity else {
        return Err(invalid());
    };
    let status = ticket.credential.as_ref().ok_or_else(invalid)?;
    let expected = basis(status);
    let result = match ticket.kind {
        Kind::Credential(Change::Set) => {
            client
                .set_credential(SetCredentialInput {
                    locator: locator(row),
                    expected: expected.map(|basis| CredentialIdentityBasis {
                        credential_id: basis.credential_id,
                        revision: basis.revision,
                    }),
                    expected_connection: Some(ConnectionCredentialTarget {
                        connection_id: row.id.clone(),
                        revision: row.revision,
                        slug: row.slug.clone(),
                        provider_type: row.provider.clone(),
                        effective_base_url: address(row).ok_or_else(invalid)?,
                    }),
                    secret: secret.ok_or_else(invalid)?,
                })
                .await?
        }
        Kind::Credential(Change::Clear) => {
            client
                .delete_credential(DeleteCredentialInput {
                    expected: expected.ok_or_else(invalid)?,
                })
                .await?
        }
        _ => return Err(invalid()),
    };
    Ok(Updated::Credential(result))
}
impl App {
    pub fn credential_request(&mut self) -> Option<Request> {
        if self.management.credential_pending.is_some() || self.management.pending.is_some() {
            return None;
        }
        let dialog = self.management.dialog.as_ref()?;
        if dialog.blocked || !self.management_identity(&dialog.target) {
            return None;
        }
        let dialog = self.management.dialog.as_mut()?;
        let state = dialog.credentials.as_mut()?;
        if !std::mem::take(&mut state.requested) {
            return None;
        }
        let request = Request {
            generation: state.generation,
            target: dialog.target.clone(),
        };
        self.management.credential_pending = Some(request.clone());
        Some(request)
    }
    pub fn credential_completed(
        &mut self,
        request: Request,
        result: Result<CredentialVaultQueryResult, RequestFailure>,
    ) {
        if self.management.credential_pending.as_ref() != Some(&request) {
            return;
        }
        self.management.credential_pending = None;
        if !self.management_identity(&request.target) {
            return;
        }
        let Some(dialog) = self
            .management
            .dialog
            .as_mut()
            .filter(|d| d.target == request.target && !d.blocked)
        else {
            return;
        };
        let Some(state) = dialog
            .credentials
            .as_mut()
            .filter(|s| s.generation == request.generation)
        else {
            return;
        };
        match result {
            Ok(CredentialVaultQueryResult::Status { status }) => {
                state.status = Some(status);
                state.failed = false;
                dialog.error = None;
            }
            Ok(CredentialVaultQueryResult::ConnectionNotFound) => {
                dialog.blocked = true;
                dialog.editor = editor();
                dialog.error = Some("credential-missing");
            }
            Err(_) => {
                state.failed = true;
                dialog.error = Some("credential-load-failed");
            }
        }
    }
    pub(super) fn credential_can_save(&self) -> bool {
        let Some(dialog) = &self.management.dialog else {
            return false;
        };
        let Some(state) = &dialog.credentials else {
            return true;
        };
        let Some(status) = &state.status else {
            return false;
        };
        match dialog.kind {
            Kind::Credential(Change::Clear) => {
                matches!(status.state, CredentialState::Configured { .. })
            }
            Kind::Credential(Change::Set) => {
                !dialog.editor.text().trim().is_empty()
                    && matches!(&dialog.target.entity,Entity::Connection(row) if address(row).is_some())
            }
            _ => false,
        }
    }
    pub fn take_credential_secret(&mut self, ticket: &Ticket) -> Option<String> {
        if ticket.kind != Kind::Credential(Change::Set)
            || self.management.pending.as_ref() != Some(ticket)
        {
            return None;
        }
        let dialog = self.management.dialog.as_mut()?;
        if dialog.editor.text().is_empty() {
            return None;
        }
        let secret = dialog.editor.text().to_owned();
        dialog.editor = editor(); // Discard the undo/redo history as well.
        Some(secret)
    }
    pub(super) fn credential_retry_enabled(&self) -> bool {
        self.management.dialog.as_ref().is_some_and(|d| {
            d.visible
                && !d.blocked
                && self.management_identity(&d.target)
                && d.credentials.as_ref().is_some_and(|s| s.failed)
        }) && self.management.pending.is_none()
            && self.management.credential_pending.is_none()
    }
    pub(super) fn credential_retry(&mut self) {
        if let Some(dialog) = &mut self.management.dialog
            && let Some(state) = &mut dialog.credentials
        {
            state.failed = false;
            state.requested = true;
            dialog.error = None;
        }
        self.hits.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{Action, ConnectionState},
        i18n::{I18n, Locale, LocalePreference},
        navigation::Route,
    };
    use crossterm::event::Event;
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;
    const KEY: &str = "synthetic-private-key";
    fn configured(locator: CredentialLocator) -> CredentialVaultQueryResult {
        CredentialVaultQueryResult::Status {
            status: CredentialStatus {
                locator,
                state: CredentialState::Configured {
                    credential_id: "fe26c818-0e6a-47ce-861c-e8c28f053bbd".into(),
                    revision: 7,
                    updated_at: 10,
                },
            },
        }
    }
    #[test]
    fn credential_form_masks_drafts_binds_reads_and_discards_uncertain_secrets() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Visit(Route::Connections));
        app.connections.refresh();
        app.connections.query().unwrap();
        app.connections.complete(Ok(json!({"kind":"page","revision":1,"connectionCount":1,"defaultTarget":null,"nextCursor":null,"items":[
            {"kind":"connection","connectionIndex":0,"connectionId":"b746eb13-287c-4f3a-8590-dac93c0a1253","revision":2,"slug":"fixture","name":"Fixture","providerType":"openai-compatible","enabled":true,"enabledModelIdCount":0,"baseUrl":"http://127.0.0.1:9/v1"}
        ]})));
        let original_row = app.connections.rows[0].clone();
        let open = app
            .management_commands()
            .into_iter()
            .find(|(_, key)| *key == "credential-key")
            .unwrap()
            .0;
        app.apply(open.clone());
        let old = app.credential_request().unwrap();
        app.apply(Action::Manage(Command::Close));
        app.apply(open.clone());
        assert!(app.credential_request().is_none());
        let status = configured(old.locator());
        app.credential_completed(old, Ok(status.clone()));
        let read = app.credential_request().unwrap();
        app.credential_completed(
            read,
            Err(RequestFailure::NotDispatched(ClientError::Timeout)),
        );
        let mut screen = Terminal::new(TestBackend::new(80, 24)).unwrap();
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        assert!(
            app.credential_request().is_none(),
            "failed reads do not spin"
        );
        assert!(app.management_enabled(&Command::CredentialRetry));
        app.apply(Action::Manage(Command::CredentialRetry));
        let read = app.credential_request().unwrap();
        app.credential_completed(read, Ok(status));
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        app.input(Event::Paste(KEY.into()));
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            for (width, height) in [(80, 24), (44, 24), (25, 8)] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|f| super::draw(f, &mut app, f.area(), ratatui::style::Style::default()))
                    .unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect();
                assert!(!text.contains(KEY));
                if width >= 44 {
                    assert!(text.contains("***"));
                    let compact = |s: &str| {
                        s.chars()
                            .filter(|c| !c.is_whitespace() && !"│─╭╮╰╯".contains(*c))
                            .collect::<String>()
                    };
                    assert!(
                        compact(&text).contains(&compact(&app.i18n.text("credential-set-note")))
                    );
                }
                assert_eq!(app.management_enabled(&Command::Save), width >= 44);
                assert!(app.i18n.diagnostics().is_empty());
            }
        }
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let ticket = app.management_request().unwrap();
        assert!(ticket.text.is_empty());
        assert!(!format!("{ticket:?}").contains(KEY));
        assert_eq!(app.take_credential_secret(&ticket).as_deref(), Some(KEY));
        assert!(app.take_credential_secret(&ticket).is_none());
        assert_eq!(
            app.management
                .dialog
                .as_ref()
                .unwrap()
                .editor
                .retained_bytes(),
            0
        );
        app.management_completed(ticket, Err(RequestFailure::Unknown(ClientError::Timeout)));
        assert!(!app.management_enabled(&Command::Save));
        app.apply(Action::Manage(Command::Close));
        // A fresh open after disconnect must not restore input or undo history.
        app.connections.rows = vec![original_row.clone()];
        app.connections.selected = Some(original_row.id.clone());
        app.apply(open);
        screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        app.input(Event::Paste(KEY.into()));
        assert_eq!(app.management.dialog.as_ref().unwrap().editor.text(), KEY);
        app.abandon_management();
        assert_eq!(
            app.management
                .dialog
                .as_ref()
                .unwrap()
                .editor
                .retained_bytes(),
            0
        );
    }
}
