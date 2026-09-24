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

use crate::i18n::I18n;
use maka_protocol::connection_effects::{
    ConnectionEffectFailureClass as Failure, ConnectionEffectRejectionReason as Rejected,
    ConnectionTestProjection as Test,
};

pub(super) fn rejection(reason: Rejected) -> &'static str {
    match reason {
        Rejected::CredentialNotConfigured => "connection-models-fetch-key",
        Rejected::ConnectionNotFound | Rejected::ConnectionDisabled => {
            "connection-test-unavailable"
        }
        Rejected::ProviderActionUnavailable => "connection-test-unavailable",
    }
}
pub(super) fn text(test: &Test, i18n: &I18n) -> String {
    match test {
        Test::Verified {
            model_id,
            latency_ms,
            ..
        } => format!(
            "{}\n{} · {} ms",
            i18n.text("connection-test-verified"),
            crate::view::safe(model_id),
            latency_ms
        ),
        Test::Failed {
            error_class,
            status_code,
            ..
        } => {
            let key = match error_class {
                Failure::Auth => "connection-test-auth",
                Failure::Timeout => "connection-test-timeout",
                Failure::InvalidResponse => "connection-test-invalid",
                Failure::ProviderUnavailable | Failure::Network | Failure::Unknown => {
                    "connection-test-failed"
                }
            };
            let text = i18n.text(key);
            status_code.map_or(text.clone(), |code| format!("{text}\nHTTP {code}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Locale, LocalePreference,
        app::{Action, App, ConnectionState},
        navigation::Route,
        pages::manage::{Command, Kind, Updated, connection::Change},
    };
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use maka_protocol::configuration::ConnectionVersionBasis;
    use maka_protocol::connection_effects::ConnectionTestRunResult;
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    #[test]
    fn test_result_is_localized_read_only_and_never_confuses_saved_failure_with_verification() {
        for locale in Locale::ALL {
            for outcome in [
                Test::Verified {
                    checked_at: "now".into(),
                    model_id: "model 中文".into(),
                    latency_ms: 12,
                },
                Test::Failed {
                    checked_at: "now".into(),
                    model_id: None,
                    latency_ms: None,
                    status_code: Some(401),
                    error_class: Failure::Auth,
                },
            ] {
                let mut app = App::new(
                    "/fixture".into(),
                    I18n::new(LocalePreference::Explicit(locale), locale),
                );
                app.connection = ConnectionState::Connected {
                    root_id: "root".into(),
                    epoch: "epoch".into(),
                };
                app.apply(Action::Visit(Route::Connections));
                app.connections.query().unwrap();
                app.connections.complete(Ok(json!({"kind":"page","revision":1,"connectionCount":1,"nextCursor":null,"defaultTarget":null,
                    "items":[{"kind":"connection","connectionIndex":0,"connectionId":"id","revision":1,"slug":"fixture","name":"Fixture","providerType":"openai-compatible","enabled":true,"enabledModelIdCount":0}]})));
                let open = app
                    .management_commands()
                    .into_iter()
                    .find(|(a, _)| {
                        matches!(
                            a,
                            Action::Manage(Command::Open(_, Kind::Connection(Change::Test)))
                        )
                    })
                    .unwrap()
                    .0;
                app.apply(open.clone());
                let mut screen = Terminal::new(TestBackend::new(80, 24)).unwrap();
                screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                app.input(Event::Key(KeyEvent::new(
                    KeyCode::Enter,
                    KeyModifiers::NONE,
                )));
                assert!(
                    app.management.dialog.is_none(),
                    "confirmation defaults to cancel"
                );
                app.apply(open);
                screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                let ticket = app.management_request().unwrap();
                app.management_completed(
                    ticket,
                    Ok(Updated::ConnectionTest(
                        ConnectionTestRunResult::Committed {
                            catalog_revision: 2,
                            connection: ConnectionVersionBasis {
                                connection_id: "id".into(),
                                revision: 2,
                            },
                            test: outcome.clone(),
                        },
                    )),
                );
                assert_eq!(
                    app.management.dialog.as_ref().unwrap().connection_test,
                    Some(outcome.clone())
                );
                assert!(
                    !app.management_enabled(&Command::Save),
                    "terminal result cannot re-run a network test"
                );
                for (width, height) in [(45, 20), (80, 24), (120, 40)] {
                    screen = Terminal::new(TestBackend::new(width, height)).unwrap();
                    screen.draw(|f| crate::view::draw(f, &mut app)).unwrap();
                    assert!(app.management.dialog.as_ref().unwrap().visible);
                    let text: String = screen
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect();
                    let compact = |s: &str| s.split_whitespace().collect::<String>();
                    assert!(
                        compact(&text).contains(&compact(&app.i18n.text("connection-test-close")))
                            && !compact(&text).contains(&compact(&app.i18n.text("session-cancel"))),
                        "a finished test offers only Close: {text}"
                    );
                }
                assert!(app.i18n.diagnostics().is_empty());
                app.input(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
                app.input(Event::Key(KeyEvent::new(
                    KeyCode::Enter,
                    KeyModifiers::NONE,
                )));
                assert!(app.management.dialog.is_none());
            }
        }
    }
}
