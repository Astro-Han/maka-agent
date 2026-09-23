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
use crate::i18n::{I18n, Locale, LocalePreference};
use ratatui::{Terminal, backend::TestBackend};
fn app() -> App {
    let mut app = App::new(
        "/unused".into(),
        I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
    );
    app.connection = ConnectionState::Connected {
        root_id: "root".into(),
        epoch: "epoch".into(),
    };
    app.apply(Action::Visit(Route::Session("source".into())));
    app
}
fn draw(app: &mut App, width: u16, height: u16) {
    Terminal::new(TestBackend::new(width, height))
        .unwrap()
        .draw(|f| crate::view::draw(f, app))
        .unwrap();
}
fn loaded(app: &mut App) {
    let action = app.recap_commands()[0].0.clone();
    app.apply(action);
    let read = app.recap_request().unwrap();
    app.recap_completed(read, Ok(None));
    draw(app, 100, 35);
}
#[test]
fn unknown_result_retries_original_after_reconnect_and_rejects_late_reply() {
    let mut app = app();
    loaded(&mut app);
    app.apply(Action::Recap(Command::Generate));
    let first = app.recap_request().unwrap();
    assert!(first.needs_checkpoint());
    assert!(app.recap_after_checkpoint(&first, &Ok(())));
    assert!(
        !app.recap_after_checkpoint(&first, &Ok(())),
        "dispatch once per checkpoint"
    );
    app.recap.disconnect();
    app.connection = ConnectionState::Connected {
        root_id: "root".into(),
        epoch: "new".into(),
    };
    app.recap_completed(
        first.clone(),
        Ok(Some(Receipt::Ready {
            operation_id: first.operation.unwrap(),
            text: "late".into(),
            model_id: "model".into(),
        })),
    );
    assert!(app.recap.receipt.is_none());
    app.apply(Action::Recap(Command::Close));
    app.apply(Action::Recap(Command::Resume));
    let read = app.recap_request().unwrap();
    app.recap_completed(read, Ok(None));
    draw(&mut app, 100, 35);
    assert!(!app.recap_enabled(&Command::Generate));
    app.apply(Action::Recap(Command::Retry));
    let retry = app.recap_request().unwrap();
    assert_eq!(retry.operation, first.operation);
    assert_ne!(retry.target.epoch, first.target.epoch);
    assert!(app.recap_after_checkpoint(&retry, &Ok(())));
    app.recap_completed(
        retry.clone(),
        Ok(Some(Receipt::Pending {
            operation_id: retry.operation.unwrap(),
        })),
    );
    assert!(
        app.recap.checkpoint().is_none(),
        "Host receipt confirms admission even if its outcome is unknown"
    );
    assert!(
        app.recap_request().is_none(),
        "receipt never starts another generation"
    );
}
#[test]
fn resize_and_failed_checkpoint_cannot_dispatch_and_foreign_checkpoint_is_rejected() {
    let mut app = app();
    loaded(&mut app);
    draw(&mut app, 30, 8);
    assert!(!app.recap_enabled(&Command::Generate));
    app.apply(Action::Recap(Command::Generate));
    assert!(app.recap_request().is_none());
    draw(&mut app, 100, 35);
    app.apply(Action::Recap(Command::Generate));
    let request = app.recap_request().unwrap();
    assert!(!app.recap_after_checkpoint(&request, &Err("disk full".into())));
    let saved = app.recap.checkpoint().unwrap();
    assert!(saved.validate("other").is_err());
    let mut reopened = State::default();
    reopened.restore(saved.clone());
    assert!(!reopened.visible);
    assert!(reopened.requested.is_none());
    assert_eq!(reopened.checkpoint(), Some(saved));
    assert!(!app.recap_enabled(&Command::Generate));
}
#[test]
fn different_latest_receipt_does_not_erase_original_unknown_request() {
    let mut app = app();
    loaded(&mut app);
    app.apply(Action::Recap(Command::Generate));
    let original = app.recap_request().unwrap();
    assert!(app.recap_after_checkpoint(&original, &Ok(())));
    app.recap_completed(original.clone(), Err(invalid("lost reply")));
    draw(&mut app, 100, 35);
    app.apply(Action::Recap(Command::Read));
    let read = app.recap_request().unwrap();
    app.recap_completed(
        read,
        Ok(Some(Receipt::Ready {
            operation_id: Uuid::new_v4(),
            text: "newer from another client".into(),
            model_id: "model".into(),
        })),
    );
    assert_eq!(
        app.recap.checkpoint().unwrap().operation,
        original.operation.unwrap()
    );
    assert!(!app.recap_enabled(&Command::Generate));
}

#[test]
fn abandoning_an_unavailable_retry_requires_explicit_confirmation_and_never_dispatches() {
    let mut app = app();
    loaded(&mut app);
    app.apply(Action::Recap(Command::Generate));
    let request = app.recap_request().unwrap();
    assert!(app.recap_after_checkpoint(&request, &Ok(())));
    app.recap_completed(request, Err(invalid("Session unavailable")));
    draw(&mut app, 100, 35);
    assert!(!app.recap_enabled(&Command::ConfirmForget));
    app.apply(Action::Recap(Command::Forget));
    assert!(app.recap.checkpoint().is_some());
    app.apply(Action::Recap(Command::ConfirmForget));
    assert!(app.recap.checkpoint().is_none());
    assert!(app.recap_request().is_none());
    assert!(!app.recap.visible);
}
