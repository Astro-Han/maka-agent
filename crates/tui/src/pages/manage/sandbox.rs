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

pub mod defaults;
mod view;
use super::{Command as Manage, Entity, Kind, Target};
use crate::app::{Action, App};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use maka_protocol::session::{ApprovalPolicy, SandboxMode};
use maka_sandbox::ApprovalKind;
pub(super) use view::draw;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Mode(SandboxMode),
    Approval(ApprovalPolicy),
    Category(ApprovalKind),
    Approvals(bool),
    Bypass,
}
impl Command {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mode(mode) => label(mode),
            Self::Approval(policy) => approval_label(policy),
            Self::Category(kind) => match kind {
                ApprovalKind::Sandbox => "session-approval-sandbox",
                ApprovalKind::Rules => "session-approval-rules",
                ApprovalKind::Permissions => "session-approval-permissions",
                ApprovalKind::Client => "session-approval-client",
            },
            Self::Approvals(true) => "session-approval-title",
            Self::Approvals(false) => "session-sandbox-change",
            Self::Bypass => "chat-sandbox-bypass",
        }
    }
}
pub(super) struct State {
    pub defaults: Option<defaults::State>,
    initial_mode: SandboxMode,
    initial_approval: ApprovalPolicy,
    pub mode: SandboxMode,
    pub approval: ApprovalPolicy,
    approvals: bool,
}
impl State {
    pub fn initial_focus(&self) -> usize {
        let command = if bypass(self.mode, self.approval) && self.defaults.is_none() {
            Command::Bypass
        } else {
            Command::Mode(self.mode)
        };
        self.controls()
            .iter()
            .position(|candidate| *candidate == command)
            .unwrap_or(0)
    }
    pub fn changed(&self) -> bool {
        self.loaded() && (self.mode != self.initial_mode || self.approval != self.initial_approval)
    }
    pub fn loaded(&self) -> bool {
        self.defaults
            .as_ref()
            .is_none_or(|state| state.revision.is_some())
    }
    pub fn mode_patch(&self) -> Option<SandboxMode> {
        (self.mode != self.initial_mode).then_some(self.mode)
    }
    pub fn approval_patch(&self) -> Option<ApprovalPolicy> {
        (self.approval != self.initial_approval).then_some(self.approval)
    }
    pub fn for_target(app: &App, target: &Target) -> Option<Self> {
        let Entity::Session { id, revision, .. } = &target.entity else {
            return None;
        };
        let detail = match &app.sessions.detail {
            crate::pages::sessions::Detail::Ready(item) => Some(item.as_ref()),
            _ => None,
        };
        let item = detail
            .into_iter()
            .chain(app.sessions.items.iter())
            .chain(app.inbox.items.iter())
            .find(|item| item.id == *id && item.revision == *revision)?;
        Some(Self {
            defaults: None,
            initial_mode: item.sandbox_mode,
            initial_approval: item.approval_policy,
            mode: item.sandbox_mode,
            approval: item.approval_policy,
            approvals: false,
        })
    }
    fn controls(&self) -> Vec<Command> {
        if !self.loaded() {
            return vec![];
        }
        if self.defaults.is_some() {
            return [
                SandboxMode::ReadOnly,
                SandboxMode::WorkspaceWrite,
                SandboxMode::DangerFullAccess,
            ]
            .map(Command::Mode)
            .to_vec();
        }
        if !self.approvals {
            return vec![
                Command::Mode(SandboxMode::ReadOnly),
                Command::Mode(SandboxMode::WorkspaceWrite),
                Command::Mode(SandboxMode::DangerFullAccess),
                Command::Approvals(true),
                Command::Bypass,
            ];
        }
        let granular = match self.approval {
            policy @ ApprovalPolicy::Granular { .. } => policy,
            _ => ApprovalPolicy::Granular {
                sandbox: false,
                rules: false,
                permissions: false,
                client: false,
            },
        };
        let mut controls = vec![
            Command::Approvals(false),
            Command::Approval(ApprovalPolicy::OnRequest),
            Command::Approval(ApprovalPolicy::Never),
            Command::Approval(granular),
        ];
        if matches!(self.approval, ApprovalPolicy::Granular { .. }) {
            controls.extend(
                [
                    ApprovalKind::Sandbox,
                    ApprovalKind::Rules,
                    ApprovalKind::Permissions,
                    ApprovalKind::Client,
                ]
                .map(Command::Category),
            );
        }
        controls
    }
    fn selected(&self, command: Command) -> bool {
        match command {
            Command::Mode(mode) => mode == self.mode,
            Command::Approval(policy) => policy == self.approval,
            Command::Category(kind) => self.approval.allows(kind),
            Command::Bypass => bypass(self.mode, self.approval),
            Command::Approvals(_) => false,
        }
    }
    fn apply(&mut self, command: Command) {
        match command {
            Command::Mode(mode) => self.mode = mode,
            Command::Approval(policy) => self.approval = policy,
            Command::Approvals(approvals) => self.approvals = approvals,
            Command::Bypass => {
                self.mode = SandboxMode::DangerFullAccess;
                self.approval = ApprovalPolicy::Never;
            }
            Command::Category(kind) => {
                if let ApprovalPolicy::Granular {
                    sandbox,
                    rules,
                    permissions,
                    client,
                } = &mut self.approval
                {
                    let enabled = match kind {
                        ApprovalKind::Sandbox => sandbox,
                        ApprovalKind::Rules => rules,
                        ApprovalKind::Permissions => permissions,
                        ApprovalKind::Client => client,
                    };
                    *enabled = !*enabled;
                }
            }
        }
    }
}
pub(crate) fn label(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::ReadOnly => "chat-sandbox-read",
        SandboxMode::WorkspaceWrite => "chat-sandbox-workspace",
        SandboxMode::DangerFullAccess => "chat-sandbox-none",
    }
}
pub(crate) fn current_label(mode: SandboxMode, approval: ApprovalPolicy) -> &'static str {
    if bypass(mode, approval) {
        "chat-sandbox-bypass"
    } else {
        label(mode)
    }
}
fn bypass(mode: SandboxMode, approval: ApprovalPolicy) -> bool {
    mode == SandboxMode::DangerFullAccess && approval == ApprovalPolicy::Never
}
fn approval_label(approval: ApprovalPolicy) -> &'static str {
    match approval {
        ApprovalPolicy::OnRequest => "session-approval-request",
        ApprovalPolicy::Never => "session-approval-never",
        ApprovalPolicy::Granular { .. } => "session-approval-granular",
    }
}
impl App {
    pub fn sandbox_disabling(&self) -> bool {
        self.management
            .dialog
            .as_ref()
            .and_then(|d| d.sandbox.as_ref())
            .is_some_and(|state| state.mode == SandboxMode::DangerFullAccess && state.changed())
    }
    pub fn sandbox_action(&self) -> Option<Action> {
        self.management_commands()
            .into_iter()
            .find_map(|(action, _)| {
                matches!(action, Action::Manage(Manage::Open(_, Kind::Sandbox))).then_some(action)
            })
    }
    pub(super) fn sandbox_update(&mut self, command: Command) {
        let dialog = self.management.dialog.as_mut().unwrap();
        let state = dialog.sandbox.as_mut().unwrap();
        let focus = state
            .controls()
            .iter()
            .position(|item| *item == command)
            .unwrap_or(0);
        state.apply(command);
        dialog.focus = if matches!(command, Command::Approvals(_)) {
            0
        } else {
            focus
        };
        dialog.error = None;
        dialog.visible = false;
        self.hits.clear();
    }
    pub(super) fn sandbox_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let dialog = self.management.dialog.as_mut().unwrap();
        let controls = dialog.sandbox.as_ref().unwrap().controls();
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Esc => Some(Manage::Close),
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                _ if !dialog.visible => None,
                KeyCode::Tab | KeyCode::Down => {
                    dialog.focus = (dialog.focus + 1) % (controls.len() + 2);
                    return (true, None);
                }
                KeyCode::BackTab | KeyCode::Up => {
                    dialog.focus = (dialog.focus + controls.len() + 1) % (controls.len() + 2);
                    return (true, None);
                }
                KeyCode::Enter | KeyCode::Char(' ') => Some(match dialog.focus {
                    index if index < controls.len() => Manage::Sandbox(controls[index]),
                    index if index == controls.len() => Manage::Close,
                    _ => Manage::Save,
                }),
                _ => None,
            },
            Event::Mouse(mouse)
                if dialog.visible && mouse.kind == MouseEventKind::Down(MouseButton::Left) =>
            {
                self.hits
                    .iter()
                    .rev()
                    .find(|hit| hit.area.contains((mouse.column, mouse.row).into()))
                    .and_then(|hit| {
                        if let Action::Manage(command) = &hit.action {
                            Some(command.clone())
                        } else {
                            None
                        }
                    })
            }
            _ => None,
        };
        (
            command.is_some(),
            command.and_then(|command| self.apply(Action::Manage(command))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Locale, LocalePreference, app::ConnectionState, i18n::I18n, navigation::Route};
    use crossterm::event::{KeyEvent, MouseEvent};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn sandbox_confirmation_is_visible_scoped_and_never_replays_unknown_changes() {
        for locale in Locale::ALL {
            let mut app = App::new(
                "/unused".into(),
                I18n::new(LocalePreference::Explicit(locale), locale),
            );
            app.connection = ConnectionState::Connected {
                root_id: "root".into(),
                epoch: "epoch".into(),
            };
            app.apply(Action::Visit(Route::Session("chat".into())));
            app.sessions.detail = crate::pages::sessions::Detail::Ready(Box::new(maka_protocol::session::decode_session_catalog_projection(&serde_json::json!({
                "id":"chat","revision":7,"workspace":{"target":{"kind":"host_path","path":"/work"},"hostCwd":"/work"},
                "createdAt":0,"activityAt":1,"name":"Session","isFlagged":false,"isArchived":false,
                "labels":[],"labelsTruncated":false,"hasUnread":false,"status":"active","backend":"ai-sdk",
                "llmConnectionId":"connection","llmConnectionSlug":"default","connectionLocked":false,"model":"model",
                "sandboxMode":"read-only","approvalPolicy":{"kind":"on-request"},"collaborationMode":"agent","orchestrationMode":"default"
            })).unwrap()));
            app.drafts.get_mut("chat").unwrap().insert("keep draft");
            let open = app.sandbox_action().unwrap();
            app.apply(open.clone());
            let mut terminal = Terminal::new(TestBackend::new(70, 32)).unwrap();
            terminal
                .draw(|frame| crate::view::draw(frame, &mut app))
                .unwrap();
            assert!(!app.management_enabled(&Manage::Save));
            app.apply(Action::Manage(Manage::Sandbox(Command::Bypass)));
            assert!(
                app.management_request().is_none(),
                "new warning has not been displayed"
            );
            terminal
                .draw(|frame| crate::view::draw(frame, &mut app))
                .unwrap();
            assert!(app.management_enabled(&Manage::Save));
            app.input(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            )));
            assert!(
                app.management.pending.is_none(),
                "Enter on the choice is not confirmation"
            );
            let mut tiny = Terminal::new(TestBackend::new(35, 10)).unwrap();
            tiny.draw(|frame| crate::view::draw(frame, &mut app))
                .unwrap();
            assert!(app.management_request().is_none());
            terminal
                .draw(|frame| crate::view::draw(frame, &mut app))
                .unwrap();
            app.input(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            }));
            assert!(app.management.dialog.is_none());
            app.apply(open);
            terminal
                .draw(|frame| crate::view::draw(frame, &mut app))
                .unwrap();
            app.apply(Action::Manage(Manage::Sandbox(Command::Bypass)));
            terminal
                .draw(|frame| crate::view::draw(frame, &mut app))
                .unwrap();
            let ticket = app.management_request().unwrap();
            assert_eq!(ticket.sandbox_mode, Some(SandboxMode::DangerFullAccess));
            assert_eq!(ticket.approval_policy, Some(ApprovalPolicy::Never));
            assert!(matches!(
                ticket.target.entity,
                Entity::Session { revision: 7, .. }
            ));
            app.management_completed(
                ticket,
                Err(maka_client::RequestFailure::Unknown(
                    maka_client::ClientError::Timeout,
                )),
            );
            assert!(app.management_request().is_none());
            assert_eq!(app.drafts["chat"].text(), "keep draft");
        }
    }
}
