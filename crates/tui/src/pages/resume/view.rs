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

use super::{App, Command};
use crate::{
    app::Action,
    view::{button, safe},
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use maka_protocol::turn::{TurnResumeParkReason, TurnResumePlan};
use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::Style,
    widgets::{Block, Paragraph, Wrap},
};

impl App {
    pub fn resume_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let command = match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return (true, Some(Action::Quit));
                }
                match key.code {
                    KeyCode::Esc => Some(Command::Close),
                    KeyCode::Tab | KeyCode::Right => {
                        self.resume.focus = (self.resume.focus + 1) % self.resume_buttons().len();
                        None
                    }
                    KeyCode::BackTab | KeyCode::Left => {
                        self.resume.focus = (self.resume.focus + self.resume_buttons().len() - 1)
                            % self.resume_buttons().len();
                        None
                    }
                    KeyCode::Enter => Some(self.resume_buttons()[self.resume.focus].clone()),
                    _ => None,
                }
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                self.hits.iter().find_map(|hit| match &hit.action {
                    Action::Resume(command)
                        if hit.area.contains((mouse.column, mouse.row).into()) =>
                    {
                        Some(command.clone())
                    }
                    _ => None,
                })
            }
            _ => None,
        };
        (
            true,
            command.and_then(|command| self.apply(Action::Resume(command))),
        )
    }

    fn resume_buttons(&self) -> Vec<Command> {
        if self.resume.saved.is_some() {
            vec![Command::Close, Command::Retry]
        } else if matches!(self.resume.plan, Some(TurnResumePlan::Ready { .. })) {
            vec![Command::Close, Command::Query, Command::Start]
        } else {
            vec![Command::Close, Command::Query]
        }
    }
}

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let width = area.width.saturating_sub(2).min(72);
    let height = area.height.saturating_sub(2).min(16);
    if width < 40 || height < 9 {
        app.resume.rendered = false;
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    app.resume.rendered = true;
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(app.i18n.text("resume-title"))
        .title_alignment(Alignment::Center)
        .style(base);
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    let mut body = String::new();
    if let Some(target) = &app.resume.target {
        body.push_str(&safe(&target.session));
        body.push_str("\n\n");
    }
    if app.resume.pending.is_some() {
        body.push_str(&app.i18n.text("resume-checking"));
    } else if app.resume.saved.is_some() {
        body.push_str(&app.i18n.text("resume-unresolved"));
    } else if let Some(plan) = &app.resume.plan {
        match plan {
            TurnResumePlan::Ready { source_turn_id, .. } => {
                body.push_str(
                    &app.i18n
                        .format("resume-ready", &[("turn", &safe(source_turn_id))]),
                );
            }
            TurnResumePlan::Parked { reason, .. } => {
                body.push_str(&app.i18n.text("resume-parked"));
                body.push('\n');
                body.push_str(&app.i18n.text(reason_key(*reason)));
            }
        }
    } else {
        body.push_str(&app.i18n.text("resume-note"));
    }
    if let Some(error) = &app.resume.error {
        body.push_str("\n\n");
        body.push_str(&safe(error));
    }
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false }),
        Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(3),
        ),
    );
    let actions = app.resume_buttons();
    app.resume.focus = app.resume.focus.min(actions.len() - 1);
    let size = inner.width / actions.len() as u16;
    for (index, command) in actions.into_iter().enumerate() {
        let label = app.i18n.text(command.label());
        button(
            frame,
            app,
            Rect::new(
                inner.x + index as u16 * size,
                inner.bottom() - 1,
                size.saturating_sub(1),
                1,
            ),
            &label,
            Action::Resume(command),
            app.resume.focus == index,
        );
    }
}

fn reason_key(reason: TurnResumeParkReason) -> &'static str {
    match reason {
        TurnResumeParkReason::ResumeCandidateMissing => "resume-reason-missing",
        TurnResumeParkReason::SourceRunUnreadable => "resume-reason-unreadable",
        TurnResumeParkReason::SafetyCheckFailed => "resume-reason-unsafe",
        TurnResumeParkReason::ContinuationAlreadyExists => "resume-reason-existing",
        TurnResumeParkReason::ContinuationRepairRequired => "resume-reason-repair",
        TurnResumeParkReason::ContinuationStartedIndeterminate => "resume-reason-unknown",
        TurnResumeParkReason::ResumeFeatureDisabled => "resume-reason-disabled",
        TurnResumeParkReason::ContinuationAuthorityUnavailable => "resume-reason-authority",
        TurnResumeParkReason::SafetyObservationUnavailable => "resume-reason-observation",
        TurnResumeParkReason::SessionBusy => "resume-reason-busy",
    }
}
