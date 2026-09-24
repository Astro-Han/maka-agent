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

use super::{Change, Command, address};
use crate::{
    app::{Action, App},
    pages::manage::{Dialog, Entity, Kind},
    ui::{Node, Role, Sheet, Tone},
    view::safe,
};
use maka_protocol::configuration::CredentialState;

/// A connection's API key: set opens in the masked field, removal on Cancel.
pub(in crate::pages::manage) fn sheet(app: &App, dialog: &Dialog) -> Sheet<Action> {
    let busy = app.management.pending.is_some();
    let state = dialog.credentials.as_ref().expect("credential state");
    let set = dialog.kind == Kind::Credential(Change::Set);
    let Entity::Connection(row) = &dialog.target.entity else {
        unreachable!()
    };
    let label = dialog.kind.label(&dialog.target);
    let endpoint = address(row);
    // Which key: the connection, where it is used, and what is saved now.
    let mut about = vec![
        Node::text("name", vec![(safe(&dialog.target.name), Tone::Normal)]),
        Node::text(
            "address",
            vec![(
                endpoint
                    .as_deref()
                    .map(safe)
                    .unwrap_or_else(|| app.i18n.text("credential-no-address")),
                Tone::Subtle,
            )],
        ),
    ];
    if let Some(status) = &state.status {
        let label = match status.state {
            CredentialState::Absent => "credential-absent",
            CredentialState::Configured { .. } => "credential-configured",
        };
        about.push(Node::text(
            "status",
            vec![(app.i18n.text(label), Tone::Subtle)],
        ));
    }
    let mut sheet = Sheet::new(
        format!("{label}:{}", dialog.target.name),
        app.i18n.text(label),
    )
    .body(Node::column("about", about));
    if set {
        sheet = sheet.field(
            "field",
            Some(app.i18n.text("credential-new-key")),
            1,
            Action::Manage(Command::Save),
            !dialog.blocked,
        );
    }
    let (note, tone) = if busy {
        ("session-saving", Tone::Subtle)
    } else if let Some(key) = dialog.editor.error.or(dialog.error) {
        (key, Tone::Warning)
    } else if state.status.is_none() {
        ("credential-loading", Tone::Subtle)
    } else if set && endpoint.is_none() {
        ("credential-no-address", Tone::Subtle)
    } else if set {
        ("credential-set-note", Tone::Subtle)
    } else {
        ("credential-clear-note", Tone::Subtle)
    };
    sheet = sheet.text("note", &app.i18n.text(note), tone);
    if state.failed {
        sheet = sheet.button(
            "retry",
            app.i18n.text("credential-retry"),
            Role::Normal,
            Action::Manage(Command::CredentialRetry),
            app.enabled(&Action::Manage(Command::CredentialRetry)),
        );
    }
    let sheet = sheet
        .button(
            "cancel",
            app.i18n.text("session-cancel"),
            Role::Normal,
            Action::Manage(Command::Close),
            true,
        )
        .button(
            "save",
            app.i18n.text(if set {
                "credential-save"
            } else {
                "credential-remove"
            }),
            if set {
                Role::Primary
            } else {
                Role::Destructive
            },
            Action::Manage(Command::Save),
            app.enabled(&Action::Manage(Command::Save)),
        );
    if set { sheet } else { sheet.focus("cancel") }
}
