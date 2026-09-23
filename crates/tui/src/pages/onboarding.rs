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

mod input;
mod view;
pub use view::draw;

use crate::{
    app::{Action, App, ConnectionState},
    editor::Editor,
};
use maka_client::{Client, RequestFailure};
use maka_protocol::configuration::{ModelInfo, onboarding::*};
use std::collections::BTreeSet;

const PROVIDERS: [(&str, &str); 3] = [
    ("openai-compatible", "OpenAI-compatible"),
    ("openai", "OpenAI"),
    ("anthropic", "Anthropic"),
];
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    Close,
    Provider,
    Field(usize),
    Verify,
    Toggle(String),
    Save,
    Back,
}
impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Open => "onboard-title",
            Self::Close => "session-cancel",
            Self::Provider => "onboard-provider",
            Self::Field(_) => "onboard-title",
            Self::Verify => "onboard-verify",
            Self::Toggle(_) => "onboard-models",
            Self::Save => "onboard-save",
            Self::Back => "onboard-back",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    generation: u64,
    root: String,
    epoch: String,
    save: bool,
}
pub struct Request {
    pub ticket: Ticket,
    input: OnboardingInput,
    models: Vec<String>,
}
pub enum ResultValue {
    Verified(OnboardingVerifyResult),
    Saved(OnboardingSaveResult),
}
pub async fn execute(client: &Client, request: Request) -> Result<ResultValue, RequestFailure> {
    if request.ticket.save {
        client
            .onboard_connection(request.input, request.models)
            .await
            .map(ResultValue::Saved)
    } else {
        client
            .verify_connection(request.input)
            .await
            .map(ResultValue::Verified)
    }
}
#[derive(Default)]
pub struct Onboarding {
    pub dialog: Option<Form>,
    pending: Option<Ticket>,
    sequence: u64,
}
impl Onboarding {
    pub fn invalidate_geometry(&mut self) {
        if let Some(form) = &mut self.dialog {
            form.visible = false;
            for field in &mut form.fields {
                field.invalidate_geometry();
            }
        }
    }
}
pub struct Form {
    ticket: Ticket,
    provider: usize,
    fields: [Editor; 3],
    models: Option<Vec<ModelInfo>>,
    selected: BTreeSet<String>,
    row: usize,
    focus: usize,
    pub visible: bool,
    blocked: bool,
    error: Option<&'static str>,
}
impl Form {
    fn count(&self) -> usize {
        if self.models.is_some() { 4 } else { 6 }
    }
    fn input(&self) -> OnboardingInput {
        let optional = |value: &str| (!value.trim().is_empty()).then(|| value.trim().to_owned());
        OnboardingInput {
            target: OnboardingTarget::Create {
                provider_type: PROVIDERS[self.provider].0.into(),
                slug: None,
                name: optional(self.fields[0].text()),
            },
            base_url: optional(self.fields[1].text()),
            api_key: optional(self.fields[2].text()),
        }
    }
}
impl App {
    pub fn onboarding_enabled(&self, c: &Command) -> bool {
        let connected = matches!(self.connection, ConnectionState::Connected { .. });
        if *c == Command::Open {
            return connected
                && self.onboarding.pending.is_none()
                && self.onboarding.dialog.is_none()
                && self.management.dialog.is_none()
                && !self.interactions.visible
                && self.queue.edit.is_none();
        }
        let Some(form) = &self.onboarding.dialog else {
            return false;
        };
        if *c == Command::Close {
            return !self.onboarding.pending.as_ref().is_some_and(|p| p.save);
        }
        let identity = matches!(&self.connection,ConnectionState::Connected{root_id,epoch} if *root_id==form.ticket.root && *epoch==form.ticket.epoch);
        if !form.visible || form.blocked || !identity || self.onboarding.pending.is_some() {
            return false;
        }
        match c {
            Command::Verify => {
                form.models.is_none()
                    && !form.fields[2].text().trim().is_empty()
                    && (form.provider != 0 || !form.fields[1].text().trim().is_empty())
            }
            Command::Save => form.models.is_some() && !form.selected.is_empty(),
            Command::Toggle(id) => form
                .models
                .as_ref()
                .is_some_and(|m| m.iter().any(|m| m.id == *id)),
            Command::Back => form.models.is_some(),
            Command::Field(index) => form.models.is_none() && *index < 3,
            Command::Provider => form.models.is_none(),
            _ => false,
        }
    }
    pub fn onboarding_action(&mut self, c: Command) -> Option<Action> {
        match c {
            Command::Open => {
                let ConnectionState::Connected { root_id, epoch } = &self.connection else {
                    return None;
                };
                self.onboarding.sequence += 1;
                self.onboarding.dialog = Some(Form {
                    ticket: Ticket {
                        generation: self.onboarding.sequence,
                        root: root_id.clone(),
                        epoch: epoch.clone(),
                        save: false,
                    },
                    provider: 0,
                    fields: [
                        Editor::bounded(128, "onboard-field-invalid"),
                        Editor::bounded(2048, "onboard-field-invalid"),
                        Editor::bounded(10240, "onboard-field-invalid"),
                    ],
                    models: None,
                    selected: BTreeSet::new(),
                    row: 0,
                    focus: 0,
                    visible: false,
                    blocked: false,
                    error: None,
                });
                self.hover = None;
            }
            Command::Close => self.onboarding.dialog = None,
            Command::Verify | Command::Save => return Some(Action::Onboard(c)),
            _ => {
                let Some(f) = &mut self.onboarding.dialog else {
                    return None;
                };
                f.error = None;
                match c {
                    Command::Provider => {
                        f.provider = (f.provider + 1) % PROVIDERS.len();
                        f.focus = 0;
                    }
                    Command::Field(index) => f.focus = index + 1,
                    Command::Toggle(id) => {
                        if !f.selected.contains(&id) && f.selected.len() >= 512 {
                            f.error = Some("onboard-model-limit");
                            return None;
                        }
                        if !f.selected.remove(&id) {
                            f.selected.insert(id.clone());
                        }
                        if let Some(models) = &f.models {
                            f.row = models.iter().position(|m| m.id == id).unwrap_or(0);
                        }
                        f.focus = 0;
                    }
                    Command::Back => {
                        f.models = None;
                        f.selected.clear();
                        f.focus = 0;
                    }
                    _ => {}
                }
            }
        }
        self.hits.clear();
        None
    }
    pub fn onboarding_request(&mut self, save: bool) -> Option<Request> {
        if !self.onboarding_enabled(&if save { Command::Save } else { Command::Verify }) {
            return None;
        }
        let f = self.onboarding.dialog.as_mut()?;
        let mut ticket = f.ticket.clone();
        ticket.save = save;
        let request = Request {
            ticket: ticket.clone(),
            input: f.input(),
            models: f
                .models
                .iter()
                .flatten()
                .filter(|m| f.selected.contains(&m.id))
                .map(|m| m.id.clone())
                .collect(),
        };
        f.error = None;
        self.onboarding.pending = Some(ticket);
        Some(request)
    }
    pub fn onboarding_completed(
        &mut self,
        ticket: Ticket,
        result: Result<ResultValue, RequestFailure>,
    ) {
        if self.onboarding.pending.as_ref() != Some(&ticket) {
            return;
        }
        self.onboarding.pending = None;
        let identity = matches!(&self.connection,ConnectionState::Connected{root_id,epoch} if *root_id==ticket.root && *epoch==ticket.epoch);
        if !identity {
            return;
        }
        let Some(f) = self
            .onboarding
            .dialog
            .as_mut()
            .filter(|f| f.ticket.generation == ticket.generation)
        else {
            return;
        };
        match result {
            Ok(ResultValue::Verified(OnboardingVerifyResult::Verified { models })) => {
                f.models = Some(models);
                f.selected.clear();
                f.row = 0;
                f.focus = 0;
                f.error = None;
            }
            Ok(ResultValue::Saved(OnboardingSaveResult::Saved { .. })) => {
                self.onboarding.dialog = None;
                self.models_catalog_changed();
                self.connections.refresh();
                self.chat.context.refresh();
            }
            Ok(ResultValue::Verified(OnboardingVerifyResult::Rejected { reason }))
            | Ok(ResultValue::Saved(OnboardingSaveResult::Rejected { reason })) => {
                f.error = Some(match reason {
                    OnboardingRejection::CredentialNotConfigured => "onboard-key-required",
                    OnboardingRejection::BaseUrlNotConfigured => "onboard-url-required",
                    OnboardingRejection::ProviderUnsupported => "onboard-unsupported",
                    OnboardingRejection::ModelUnavailable => "onboard-models-changed",
                    OnboardingRejection::CatalogFull => "onboard-catalog-full",
                    OnboardingRejection::Superseded => "onboard-conflict",
                    _ => "onboard-request-failed",
                });
            }
            Ok(ResultValue::Verified(OnboardingVerifyResult::Failed { error_class }))
            | Ok(ResultValue::Saved(OnboardingSaveResult::Failed { error_class })) => {
                use maka_protocol::configuration::ConnectionEffectFailureClass as E;
                f.error = Some(match error_class {
                    E::Auth => "onboard-auth-failed",
                    E::InvalidResponse => "onboard-response-failed",
                    _ => "onboard-network-failed",
                });
            }
            Err(RequestFailure::Unknown(_)) if ticket.save => {
                f.blocked = true;
                f.error = Some("onboard-unknown");
            }
            Err(RequestFailure::Rejected(maka_client::ClientError::Rejected(error)))
                if ticket.save
                    && error.code == maka_protocol::OperationErrorCode::CommitOutcomeUnknown =>
            {
                f.blocked = true;
                f.error = Some("onboard-unknown");
            }
            Err(_) => f.error = Some("onboard-request-failed"),
        }
    }
    pub fn abandon_onboarding(&mut self) {
        let uncertain = self.onboarding.pending.take().is_some_and(|p| p.save);
        if let Some(f) = &mut self.onboarding.dialog {
            f.blocked = true;
            f.fields[2] = Editor::bounded(10240, "onboard-field-invalid");
            f.error = Some(if uncertain {
                "onboard-unknown"
            } else {
                "onboard-disconnected"
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{I18n, Locale, LocalePreference};
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    #[test]
    fn onboarding_masks_secrets_isolates_async_steps_and_never_replays_unknown_saves() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Onboard(Command::Open));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        for (index, text) in [
            "Private fixture",
            "http://127.0.0.1/v1",
            "fixture-SECRET-中文",
        ]
        .into_iter()
        .enumerate()
        {
            app.apply(Action::Onboard(Command::Field(index)));
            app.input(Event::Paste(text.into()));
        }
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), locale);
            for (width, height) in [(80, 24), (44, 22), (20, 8)] {
                let mut screen = Terminal::new(TestBackend::new(width, height)).unwrap();
                screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                let text = screen
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|c| c.symbol())
                    .collect::<String>();
                assert!(!text.contains("SECRET") && !text.contains("fixture-"));
                assert_eq!(app.onboarding_enabled(&Command::Verify), width >= 44);
                if width >= 44 {
                    assert!(text.contains("*****"));
                    assert!(
                        screen
                            .backend()
                            .buffer()
                            .content
                            .iter()
                            .rev()
                            .take(width as usize)
                            .all(|c| c.symbol() == " "),
                        "modal hides unrelated background footer hints"
                    );
                }
            }
        }
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let request = app.onboarding_request(false).unwrap();
        assert_eq!(
            request.input.api_key.as_deref(),
            Some("fixture-SECRET-中文")
        );
        app.input(Event::Paste("ignored while verifying".into()));
        assert!(!app.onboarding_enabled(&Command::Verify));
        app.input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(app.onboarding.dialog.is_none());
        assert!(
            !app.onboarding_enabled(&Command::Open),
            "closed verification occupies its slot until completion"
        );
        let model: ModelInfo = serde_json::from_value(json!({"id":"model"})).unwrap();
        app.onboarding_completed(
            request.ticket,
            Ok(ResultValue::Verified(OnboardingVerifyResult::Verified {
                models: vec![model.clone()],
            })),
        );
        assert!(
            app.onboarding.dialog.is_none(),
            "closed reply cannot reopen the modal"
        );
        app.apply(Action::Onboard(Command::Open));
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        for (index, text) in [(1, "http://127.0.0.1/v1"), (2, "new-secret")] {
            app.apply(Action::Onboard(Command::Field(index)));
            app.input(Event::Paste(text.into()));
        }
        let request = app.onboarding_request(false).unwrap();
        app.onboarding_completed(
            request.ticket,
            Ok(ResultValue::Verified(OnboardingVerifyResult::Verified {
                models: vec![
                    model,
                    ModelInfo {
                        id: "z-model".into(),
                        ..Default::default()
                    },
                ],
            })),
        );
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        app.input(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)));
        assert_eq!(app.onboarding.dialog.as_ref().unwrap().row, 1);
        app.input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(
            app.onboarding.dialog.as_ref().unwrap().row,
            1,
            "wheel outside the modal cannot move its model list"
        );
        app.input(Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)));
        assert!(
            !app.onboarding_enabled(&Command::Save),
            "no silent enable-all default"
        );
        let hit = app
            .hits
            .iter()
            .find(|h| h.action == Action::Onboard(Command::Toggle("model".into())))
            .unwrap()
            .area;
        app.input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(app.onboarding_enabled(&Command::Save));
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        let save_y = app
            .hits
            .iter()
            .find(|h| h.action == Action::Onboard(Command::Save))
            .unwrap()
            .area
            .y;
        assert!(
            save_y - hit.y < 10,
            "small model inventories keep a compact dialog"
        );
        app.apply(Action::Onboard(Command::Back));
        assert!(
            !app.onboarding_enabled(&Command::Save),
            "editing invalidates discovery"
        );
        let request = app.onboarding_request(false).unwrap();
        let model = serde_json::from_value(json!({"id":"model"})).unwrap();
        app.onboarding_completed(
            request.ticket,
            Ok(ResultValue::Verified(OnboardingVerifyResult::Verified {
                models: vec![model],
            })),
        );
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        )));
        let save = app.onboarding_request(true).unwrap();
        assert_eq!(save.models, ["model"]);
        assert!(
            !app.onboarding_enabled(&Command::Close),
            "closing cannot masquerade as cancelling a save"
        );
        app.onboarding_completed(
            save.ticket,
            Err(RequestFailure::Rejected(
                maka_client::ClientError::Rejected(maka_protocol::OperationError {
                    code: maka_protocol::OperationErrorCode::CommitOutcomeUnknown,
                    message: "must not display new-secret".into(),
                }),
            )),
        );
        assert!(!app.onboarding_enabled(&Command::Save) && !app.onboarding_enabled(&Command::Back));
        terminal.draw(|f| crate::view::draw(f, &mut app)).unwrap();
        assert!(
            !terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
                .contains("new-secret")
        );
        app.abandon_onboarding();
        assert!(
            app.onboarding.dialog.as_ref().unwrap().fields[2]
                .text()
                .is_empty()
        );
    }
}
