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

use super::{Command, Manage, approval_label, bypass};
use crate::{
    app::{Action, App},
    pages::manage::Dialog,
    ui::{Node, On, Role, Sheet, Tone},
};
use maka_protocol::session::{ApprovalPolicy, SandboxMode};
use maka_sandbox::ApprovalKind;

/// Isolation, then escalation approvals as a second step that Esc or its
/// back row leaves. Choosing only stages a change; Save commits it, in amber
/// when it weakens isolation.
pub(in crate::pages::manage) fn sheet(app: &App, dialog: &Dialog) -> Sheet<Action> {
    let state = dialog.sandbox.as_ref().expect("sandbox state");
    let full_bypass = bypass(state.mode, state.approval);
    let danger = state.mode == SandboxMode::DangerFullAccess;
    // Loading defaults is its own step: arriving values focus their choice.
    let (step, title) = if state.defaults.is_some() && !state.loaded() {
        ("loading", "sandbox-default-title")
    } else if state.defaults.is_some() {
        ("defaults", "sandbox-default-title")
    } else if state.approvals {
        ("approvals", "session-approval-title")
    } else {
        ("modes", "session-sandbox-change")
    };
    let mut sheet = Sheet::new(
        format!("sandbox:{}:{step}", dialog.target.name),
        app.i18n.text(title),
    );
    let enabled = !dialog.blocked && app.management.pending.is_none();
    let row = |command: Command| {
        Node::text(
            key(command),
            vec![(label(app, state, command), tone(command))],
        )
        .clip()
        .on(On::Activate(Action::Manage(Manage::Sandbox(command))))
        .enabled(enabled)
    };
    // Choices of one kind form a compact group; groups and links stand apart.
    let mut pending: Option<(&'static str, Vec<Node<Action>>)> = None;
    for command in state.controls() {
        if let Some((name, rows)) = &mut pending
            && group(command) == Some(*name)
        {
            rows.push(row(command));
            continue;
        }
        if let Some((name, rows)) = pending.take() {
            sheet = sheet.body(Node::column(name, rows));
        }
        match group(command) {
            Some(name) => pending = Some((name, vec![row(command)])),
            None => sheet = sheet.body(row(command)),
        }
    }
    if let Some((name, rows)) = pending {
        sheet = sheet.body(Node::column(name, rows));
    }
    let note = app.i18n.text(if !state.loaded() {
        "sandbox-default-loading"
    } else if state.defaults.is_some() && !danger {
        "sandbox-default-note"
    } else if full_bypass {
        "session-sandbox-bypass-warning"
    } else if danger {
        "session-sandbox-warning"
    } else if state.approvals {
        "session-approval-note"
    } else {
        "session-sandbox-note"
    });
    let tone = if danger { Tone::Warning } else { Tone::Muted };
    sheet = sheet.text("note", &note, tone);
    if state.defaults.is_some() && danger {
        sheet = sheet.text("defaults", &app.i18n.text("sandbox-default-note"), tone);
    }
    if let Some(error) = dialog.error {
        sheet = sheet.text("error", &app.i18n.text(error), Tone::Warning);
    }
    let save = if full_bypass {
        "session-sandbox-enable-bypass"
    } else if danger && state.mode_patch().is_some() {
        "session-sandbox-disable"
    } else {
        "session-save"
    };
    let mut sheet = sheet
        .button(
            "cancel",
            app.i18n.text("session-cancel"),
            Role::Normal,
            Action::Manage(Manage::Close),
            true,
        )
        .button(
            "save",
            app.i18n.text(save),
            if app.sandbox_disabling() {
                Role::Caution
            } else {
                Role::Primary
            },
            Action::Manage(Manage::Save),
            app.enabled(&Action::Manage(Manage::Save)),
        );
    if state.approvals {
        sheet = sheet.back(Action::Manage(Manage::Sandbox(Command::Approvals(false))));
    }
    match state.initial() {
        Some(command) => sheet.focus_node(path(command)),
        None => sheet,
    }
}

fn label(app: &App, state: &super::State, command: Command) -> String {
    let selected = state.selected(command);
    match command {
        Command::Approvals(true) => format!(
            "{} · {} {}",
            app.i18n.text(command.label()),
            app.i18n.text(approval_label(state.approval)),
            app.chrome.symbol("›", ">")
        ),
        Command::Approvals(false) => format!(
            "{} {}",
            app.chrome.symbol("‹", "<"),
            app.i18n.text(command.label())
        ),
        Command::Category(_) => format!(
            "{} {}",
            if selected {
                app.chrome.symbol("☑", "[x]")
            } else {
                app.chrome.symbol("☐", "[ ]")
            },
            app.i18n.text(command.label())
        ),
        _ => format!(
            "{} {}",
            if selected {
                app.chrome.symbol("●", "[x]")
            } else {
                app.chrome.symbol("○", "[ ]")
            },
            app.i18n.text(command.label())
        ),
    }
}

fn tone(command: Command) -> Tone {
    if matches!(
        command,
        Command::Bypass | Command::Mode(SandboxMode::DangerFullAccess)
    ) {
        Tone::Warning
    } else {
        Tone::Normal
    }
}

/// Stable keys: a granular policy's flags change, its row does not.
fn key(command: Command) -> &'static str {
    match command {
        Command::Mode(SandboxMode::ReadOnly) => "read-only",
        Command::Mode(SandboxMode::WorkspaceWrite) => "workspace-write",
        Command::Mode(SandboxMode::DangerFullAccess) => "full-access",
        Command::Approvals(true) => "approvals",
        Command::Approvals(false) => "back",
        Command::Bypass => "bypass",
        Command::Approval(ApprovalPolicy::OnRequest) => "on-request",
        Command::Approval(ApprovalPolicy::Never) => "never",
        Command::Approval(ApprovalPolicy::Granular { .. }) => "granular",
        Command::Category(ApprovalKind::Sandbox) => "sandbox",
        Command::Category(ApprovalKind::Rules) => "rules",
        Command::Category(ApprovalKind::Permissions) => "permissions",
        Command::Category(ApprovalKind::Client) => "client",
    }
}

fn group(command: Command) -> Option<&'static str> {
    match command {
        Command::Mode(_) => Some("modes"),
        Command::Approval(_) => Some("policies"),
        Command::Category(_) => Some("categories"),
        Command::Approvals(_) | Command::Bypass => None,
    }
}

fn path(command: Command) -> String {
    match group(command) {
        Some(group) => format!("{group}/{}", key(command)),
        None => key(command).to_owned(),
    }
}
