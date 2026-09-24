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
use crate::{app::App, pages::connections::Row};
use maka_client::{Client, ClientError, RequestFailure};
use maka_protocol::configuration::*;
pub(super) use view::sheet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Clear,
}
impl Change {
    pub fn label(self) -> &'static str {
        match self {
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
fn locator(row: &Row) -> CredentialLocator {
    CredentialLocator::Connection {
        connection_id: row.id.clone(),
        kind: ConnectionCredentialKind::Provider,
    }
}
pub(super) fn address(row: &Row) -> String {
    format!(
        "{} / {} / {} / {}",
        row.provider.package_id,
        row.provider.entry_id,
        String::from(row.provider.scope.clone()),
        row.provider.name
    )
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
pub(super) async fn execute(client: &Client, ticket: &Ticket) -> Result<Updated, RequestFailure> {
    let invalid =
        || RequestFailure::NotDispatched(ClientError::Protocol("Missing credential basis".into()));
    let status = ticket.credential.as_ref().ok_or_else(invalid)?;
    let expected = basis(status);
    let result = match ticket.kind {
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
            _ => false,
        }
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
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;
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
    fn credential_removal_uses_the_current_read_and_does_not_replay_unknown_changes() {
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
            {"kind":"connection","connectionIndex":0,"connectionId":"b746eb13-287c-4f3a-8590-dac93c0a1253","revision":2,"slug":"fixture","name":"Fixture","provider":crate::providers::fixtures::entry("openai-compatible", false).identity,"configuration":{"baseUrl":"http://127.0.0.1:9/v1"},"enabled":true,"enabledModelIdCount":0}
        ]})));
        let open = app
            .management_commands()
            .into_iter()
            .find(|(_, key)| *key == "credential-clear")
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
        assert!(app.management_enabled(&Command::Save));
        let ticket = app.management_request().unwrap();
        let status = ticket.credential.as_ref().unwrap();
        assert!(matches!(
            status.locator,
            CredentialLocator::Connection {
                kind: ConnectionCredentialKind::Provider,
                ..
            }
        ));
        assert_eq!(basis(status).unwrap().revision, 7);
        app.management_completed(ticket, Err(RequestFailure::Unknown(ClientError::Timeout)));
        assert!(!app.management_enabled(&Command::Save));
        assert!(app.credential_request().is_none());
        app.apply(Action::Manage(Command::Close));
        app.connections.query().unwrap();
        app.connections.complete(Ok(json!({"kind":"page","revision":1,"connectionCount":1,"defaultTarget":null,"nextCursor":null,"items":[
            {"kind":"connection","connectionIndex":0,"connectionId":"b746eb13-287c-4f3a-8590-dac93c0a1253","revision":2,"slug":"fixture","name":"Fixture","provider":crate::providers::fixtures::entry("openai-compatible", false).identity,"configuration":{"baseUrl":"http://127.0.0.1:9/v1"},"enabled":true,"enabledModelIdCount":0}
        ]})));
        app.apply(open);
        let request = app.credential_request().unwrap();
        app.credential_completed(request.clone(), Ok(configured(request.locator())));
        screen
            .draw(|frame| crate::view::draw(frame, &mut app))
            .unwrap();
        assert!(app.management_enabled(&Command::Save));
    }
}
