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

use super::{Action, App, Command, Input};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use maka_plugins::authorization::{Capability, Request, Target};
use maka_protocol::plugin::TerminalViewProjection;
use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::Style,
    widgets::{Block, BorderType, Paragraph},
};
use unicode_width::UnicodeWidthStr;

pub(super) struct Consent {
    pub view: Box<TerminalViewProjection>,
    pub input: Input,
    pub proposal: Request,
    pub focus: usize,
    pub rendered: bool,
}
impl App {
    pub fn extensions_consent_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                KeyCode::Esc => Some(Command::DismissConsent),
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                    self.extensions.consent.as_mut().unwrap().focus ^= 1;
                    None
                }
                KeyCode::Enter => Some(if self.extensions.consent.as_ref().unwrap().focus == 0 {
                    Command::DismissConsent
                } else {
                    Command::ApproveConsent
                }),
                _ => None,
            },
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                self.hits.iter().find_map(|hit| match &hit.action {
                    Action::Extension(
                        command @ (Command::ApproveConsent | Command::DismissConsent),
                    ) if hit.area.contains((mouse.column, mouse.row).into()) => {
                        Some(command.clone())
                    }
                    _ => None,
                })
            }
            _ => None,
        };
        (
            true,
            command.and_then(|command| self.apply(Action::Extension(command))),
        )
    }
}
pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let state = app.extensions.consent.as_ref().unwrap();
    let width = area.width.saturating_sub(4).min(68);
    let mut body = format!(
        "{}\n\n{}\n\n",
        visible(&state.view.package_id),
        target(app, &state.proposal.target)
    );
    for capability in &state.proposal.capabilities {
        body.push_str("• ");
        body.push_str(&app.i18n.text(capability_key(*capability)));
        body.push('\n');
    }
    body.push('\n');
    body.push_str(&app.i18n.text("extensions-consent-duration"));
    let cancel = app.i18n.text("session-cancel");
    let allow = app.i18n.text("extensions-authorize");
    let cancel_width = cancel.width() as u16 + 4;
    let allow_width = allow.width() as u16 + 4;
    let lines = crate::pages::manage::view::note_lines(&body, width.saturating_sub(6));
    let height = lines.len() as u16 + 6;
    app.hits.clear();
    if width < (cancel_width + allow_width + 8).max(34) || height > area.height.saturating_sub(2) {
        app.extensions.consent.as_mut().unwrap().rendered = false;
        app.modal_area = Some(area);
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).style(base),
            area.inner(Margin::new(2, 1)),
        );
        return;
    }
    let focus = state.focus;
    app.extensions.consent.as_mut().unwrap().rendered = true;
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(app.i18n.text("extensions-consent-title"))
        .title_alignment(Alignment::Center)
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().accent))
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        });
    let inner = block.inner(popup).inner(Margin::new(2, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(lines).style(base),
        Rect::new(inner.x, inner.y + 1, inner.width, height - 5),
    );
    let x = inner.x + inner.width.saturating_sub(cancel_width + allow_width + 2) / 2;
    for (x, width, label, command, selected) in [
        (x, cancel_width, cancel, Command::DismissConsent, focus == 0),
        (
            x + cancel_width + 2,
            allow_width,
            allow,
            Command::ApproveConsent,
            focus == 1,
        ),
    ] {
        crate::view::button(
            frame,
            app,
            Rect::new(x, inner.bottom() - 1, width, 1),
            &label,
            Action::Extension(command),
            selected,
        );
    }
}
fn visible(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                c.escape_unicode().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}
fn target(app: &App, target: &Target) -> String {
    match target {
        Target::Profile => app.i18n.text("extensions-target-profile"),
        Target::Directory { path } => visible(path),
        Target::Session { session_id } => format!(
            "{} · {}",
            app.i18n.text("extensions-target-session"),
            visible(session_id)
        ),
        Target::PluginWorkspace { sandbox_mode } => format!(
            "{} · {}",
            app.i18n.text("extensions-target-plugin-workspace"),
            mode(app, *sandbox_mode)
        ),
        Target::Workspace {
            workspace,
            sandbox_mode,
        } => {
            let name = match workspace {
                maka_runtime::execution::WorkspaceTarget::Project { project_id } => format!(
                    "{} · {}",
                    app.i18n.text("route-projects"),
                    visible(project_id)
                ),
                maka_runtime::execution::WorkspaceTarget::HostPath { path } => visible(path),
            };
            format!("{name}\n{}", mode(app, *sandbox_mode))
        }
    }
}
fn mode(app: &App, mode: maka_runtime::execution::SandboxMode) -> String {
    use maka_runtime::execution::SandboxMode;
    app.i18n.text(match mode {
        SandboxMode::ReadOnly => "chat-sandbox-read",
        SandboxMode::WorkspaceWrite => "chat-sandbox-workspace",
        SandboxMode::DangerFullAccess => "chat-sandbox-none",
    })
}
fn capability_key(capability: Capability) -> &'static str {
    match capability {
        Capability::ReadFiles => "extensions-cap-read-files",
        Capability::WriteFiles => "extensions-cap-write-files",
        Capability::Network => "extensions-cap-network",
        Capability::Models => "extensions-cap-models",
        Capability::Processes => "extensions-cap-processes",
        Capability::ClientCapabilities => "extensions-cap-client",
        Capability::Executions => "extensions-cap-executions",
        Capability::Notifications => "extensions-cap-notifications",
        Capability::ReadSessions => "extensions-cap-sessions",
        Capability::ReadHistory => "extensions-cap-history",
        Capability::ReadUsage => "extensions-cap-usage",
        Capability::ManagePricing => "extensions-cap-pricing",
    }
}
