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

use super::{LoginTarget, PROVIDERS, Provider, State};
use crate::editor::Editor;

pub(super) struct Identity {
    pub expanded: bool,
    pub fields: [Editor; 2],
}
impl Default for Identity {
    fn default() -> Self {
        Self {
            expanded: false,
            fields: [
                Editor::bounded(1024, "oauth-name-invalid"),
                Editor::bounded(64, "oauth-slug-invalid"),
            ],
        }
    }
}
impl Identity {
    pub fn target(&self, provider: Provider) -> LoginTarget {
        let value = |index: usize| {
            let text = self.fields[index].text().trim();
            (provider == Provider::OpenaiCodex && !text.is_empty()).then(|| text.to_owned())
        };
        LoginTarget::Create {
            provider_type: provider,
            name: value(0),
            slug: value(1),
        }
    }
    pub fn error(&self) -> Option<&'static str> {
        if let Some(error) = self.fields.iter().find_map(|field| field.error) {
            return Some(error);
        }
        let LoginTarget::Create { name, slug, .. } = self.target(Provider::OpenaiCodex) else {
            unreachable!()
        };
        for (name, slug, error) in [
            (name, None, "oauth-name-invalid"),
            (None, slug, "oauth-slug-invalid"),
        ] {
            if (LoginTarget::Create {
                provider_type: Provider::OpenaiCodex,
                name,
                slug,
            })
            .validate_create_identity()
            .is_err()
            {
                return Some(error);
            }
        }
        None
    }
    pub fn invalidate_geometry(&mut self) {
        for field in &mut self.fields {
            field.invalidate_geometry();
        }
    }
}
impl State {
    pub(super) fn customizable(&self) -> bool {
        self.attempt.is_none()
            && self.existing.is_none()
            && PROVIDERS[self.provider] == Provider::OpenaiCodex
    }
    pub fn invalidate_identity_geometry(&mut self) {
        self.identity.invalidate_geometry();
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Command, Manage, Output};
    use super::*;
    use crate::{
        app::{Action, App, ConnectionState},
        i18n::{I18n, Locale, LocalePreference},
    };
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::view::draw(frame, app))
            .unwrap();
        format!("{:?}", terminal.backend().buffer())
    }
    fn action(app: &mut App, command: Command) {
        assert!(app.oauth_enabled(command));
        app.apply(Action::Manage(Manage::Oauth(command)));
        render(app, 80, 24);
    }
    fn replace(app: &mut App, text: &str) {
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
        )));
        app.input(Event::Paste(text.into()));
    }

    #[test]
    fn optional_codex_identity_is_bounded_editable_preserved_when_hidden_and_frozen_on_start() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Auto, Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(app.oauth_commands()[0].0.clone());
        app.management.oauth.ready = true;
        app.management.oauth.enrollment = Some(true);
        render(&mut app, 80, 24);
        assert!(!app.oauth_enabled(Command::Field(0)));
        assert_eq!(
            app.management.oauth.identity.target(Provider::OpenaiCodex),
            LoginTarget::Create {
                provider_type: Provider::OpenaiCodex,
                name: None,
                slug: None
            }
        );
        action(&mut app, Command::Identity);
        for locale in [Locale::En, Locale::ZhCn, Locale::ZhTw] {
            app.i18n.preference = LocalePreference::Explicit(locale);
            for (width, height) in [(44, 20), (80, 24), (120, 40)] {
                let text = render(&mut app, width, height);
                let field_area = |index| {
                    app.hits
                        .iter()
                        .find(|hit| {
                            hit.action == Action::Manage(Manage::Oauth(Command::Field(index)))
                        })
                        .unwrap()
                        .area
                };
                assert!(field_area(1).y > field_area(0).bottom());
                let disclosure = app
                    .hits
                    .iter()
                    .find(|hit| hit.action == Action::Manage(Manage::Oauth(Command::Identity)))
                    .unwrap();
                assert!(field_area(0).y > disclosure.area.bottom());
                for index in 0..2 {
                    assert!(text.contains(&app.i18n.text(Command::Field(index).label())));
                    let hit = app
                        .hits
                        .iter()
                        .find(|hit| {
                            hit.action == Action::Manage(Manage::Oauth(Command::Field(index)))
                        })
                        .unwrap()
                        .area;
                    app.input(Event::Mouse(MouseEvent {
                        kind: MouseEventKind::Down(MouseButton::Left),
                        column: hit.x,
                        row: hit.y,
                        modifiers: KeyModifiers::NONE,
                    }));
                    assert_eq!(
                        app.management.oauth.controls()
                            [app.management.dialog.as_ref().unwrap().focus],
                        Manage::Oauth(Command::Field(index))
                    );
                    render(&mut app, width, height);
                }
            }
        }
        action(&mut app, Command::Field(0));
        replace(&mut app, "  工作账号 🦀  ");
        action(&mut app, Command::Field(1));
        replace(&mut app, "Bad ID");
        assert_eq!(
            app.management.oauth.identity.error(),
            Some("oauth-slug-invalid")
        );
        assert!(!app.oauth_enabled(Command::Begin));
        replace(&mut app, "work-codex");
        assert!(app.management.oauth.identity.error().is_none());
        app.input(Event::Paste("\nno".into()));
        assert_eq!(app.management.oauth.identity.fields[1].text(), "work-codex");
        assert!(!app.oauth_enabled(Command::Begin));
        replace(&mut app, "work-codex");
        action(&mut app, Command::Identity);
        assert!(!app.oauth_enabled(Command::Field(0)));
        let target = app.management.oauth.identity.target(Provider::OpenaiCodex);
        assert_eq!(
            target,
            LoginTarget::Create {
                provider_type: Provider::OpenaiCodex,
                name: Some("工作账号 🦀".into()),
                slug: Some("work-codex".into())
            }
        );
        action(&mut app, Command::Provider(2));
        assert!(!app.oauth_enabled(Command::Identity));
        assert_eq!(
            app.management.oauth.identity.target(Provider::XaiOauth),
            LoginTarget::Create {
                provider_type: Provider::XaiOauth,
                name: None,
                slug: None
            }
        );
        action(&mut app, Command::Provider(0));
        let request = app.oauth_request().unwrap();
        app.oauth_completed(
            request,
            Ok(Output::Enrollment(
                maka_protocol::oauth::EnrollmentProjection {
                    provider: Provider::OpenaiCodex,
                    enabled: true,
                },
            )),
        );
        render(&mut app, 80, 24);
        action(&mut app, Command::Identity);
        action(&mut app, Command::Field(0));
        replace(&mut app, &"🦀".repeat(129));
        assert_eq!(
            app.management.oauth.identity.error(),
            Some("oauth-name-invalid")
        );
        replace(&mut app, "工作账号 🦀");
        render(&mut app, 25, 8);
        app.input(Event::Paste("must not edit hidden field".into()));
        assert_eq!(
            app.management.oauth.identity.fields[0].text(),
            "工作账号 🦀"
        );
        render(&mut app, 80, 24);
        action(&mut app, Command::Begin);
        let request = app.oauth_request().unwrap();
        assert_eq!(request.start.as_ref().unwrap().target, target);
        assert!(request.needs_checkpoint());
        assert!(!app.oauth_enabled(Command::Field(0)));
        assert!(!app.oauth_enabled(Command::Identity));
        app.input(Event::Paste("late edits".into()));
        assert_eq!(
            app.management.oauth.attempt.as_ref().unwrap().target,
            target
        );
        let saved = serde_json::to_value(app.management.oauth.checkpoint()).unwrap();
        assert_eq!(saved["start"]["target"]["slug"], "work-codex");
        assert_eq!(saved["start"]["target"]["name"], "工作账号 🦀");
    }
}
