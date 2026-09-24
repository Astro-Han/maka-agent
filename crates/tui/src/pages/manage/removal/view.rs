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

use super::{Action, App, Command};
use crate::{
    pages::manage::Dialog,
    ui::{Role, Sheet, Tone},
    view::safe,
};

/// Deleting a session: Cancel is the default. A failed read offers a retry
/// instead; an edit conflict offers only closing.
pub(in crate::pages::manage) fn sheet(app: &App, dialog: &Dialog) -> Sheet<Action> {
    let busy = app.management.pending.is_some();
    let reading = app.management.removal_pending.is_some();
    let state = dialog.removal.as_ref().expect("removal state");
    let (message, tone) = if busy {
        (app.i18n.text("session-remove-wait"), Tone::Muted)
    } else if let Some(error) = state.error {
        (app.i18n.text(error), Tone::Warning)
    } else if reading || state.count.is_none() {
        (app.i18n.text("session-remove-checking"), Tone::Muted)
    } else {
        let mut note = app.i18n.text("session-remove-note");
        if let Some(count) = state.count.filter(|count| *count > 0) {
            note.push_str("\n\n");
            note.push_str(
                &app.i18n
                    .format("session-remove-subtasks", &[("count", &count.to_string())]),
            );
        }
        (note, Tone::Muted)
    };
    let error = state.error.is_some();
    let mut sheet = Sheet::new(
        format!("remove:{}", dialog.target.name),
        app.i18n.text("session-remove"),
    )
    .text("name", &safe(&dialog.target.name), Tone::Normal)
    .text("note", &message, tone)
    .button(
        "cancel",
        app.i18n.text(if busy || error {
            "session-remove-close"
        } else {
            "session-cancel"
        }),
        Role::Normal,
        Action::Manage(Command::Close),
        true,
    );
    if state.error != Some("session-edit-conflict") {
        let (label, role, command) = match (error, state.uncertain) {
            (true, true) => ("session-remove-query", Role::Normal, Command::RemovalQuery),
            (true, false) => ("session-remove-retry", Role::Normal, Command::RemovalQuery),
            _ => ("session-remove-confirm", Role::Destructive, Command::Save),
        };
        let action = Action::Manage(command);
        let enabled = app.enabled(&action);
        sheet = sheet.button("confirm", app.i18n.text(label), role, action, enabled);
    }
    sheet.focus("cancel")
}
