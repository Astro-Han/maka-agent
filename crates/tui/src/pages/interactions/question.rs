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

use super::Command;
use crate::{
    app::{Action, Hit},
    editor::Editor,
    i18n::I18n,
    pages::chat::layout,
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use maka_protocol::interaction::InteractionQuestion;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::Style,
    text::Line,
    widgets::Paragraph,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Unset,
    Option(usize),
    Text,
    Skip,
}
struct Draft {
    answer: Answer,
    editor: Editor,
}
pub struct Questions {
    questions: Vec<InteractionQuestion>,
    drafts: Vec<Draft>,
    current: usize,
    pub focus: Command,
    scroll: usize,
    max_scroll: usize,
    reveal: bool,
}
impl Questions {
    pub fn new(questions: Vec<InteractionQuestion>) -> Self {
        Self {
            drafts: questions
                .iter()
                .map(|_| Draft {
                    answer: Answer::Unset,
                    editor: Editor::bounded(2048, "question-too-large"),
                })
                .collect(),
            questions,
            current: 0,
            focus: Command::Close,
            scroll: 0,
            max_scroll: 0,
            reveal: false,
        }
    }
    pub fn reset_focus(&mut self) {
        self.focus = Command::Close;
        self.invalidate_geometry();
    }
    pub fn invalidate_geometry(&mut self) {
        for draft in &mut self.drafts {
            draft.editor.invalidate_geometry();
        }
    }
    fn controls(&self) -> Vec<Command> {
        let mut controls = vec![Command::Close];
        controls.extend((0..self.questions.len()).map(Command::Question));
        controls.extend((0..self.questions[self.current].options.len()).map(Command::Option));
        controls.extend([Command::FreeText, Command::Skip, Command::Submit]);
        controls
    }
    pub fn accepts(&self, command: Command) -> bool {
        self.controls().contains(&command)
    }
    fn answer(&self, index: usize) -> Option<Option<String>> {
        match self.drafts[index].answer {
            Answer::Unset => None,
            Answer::Skip => Some(None),
            Answer::Option(option) => Some(Some(
                self.questions[index].options.get(option)?.label.clone(),
            )),
            Answer::Text => {
                let text = self.drafts[index].editor.text();
                (!text.trim().is_empty()).then(|| Some(text.to_owned()))
            }
        }
    }
    pub fn answered(&self) -> usize {
        (0..self.drafts.len())
            .filter(|index| self.answer(*index).is_some())
            .count()
    }
    pub fn len(&self) -> usize {
        self.questions.len()
    }
    pub fn answers(&self) -> Option<Vec<Option<String>>> {
        (0..self.drafts.len())
            .map(|index| self.answer(index))
            .collect()
    }
    pub fn error(&self) -> Option<&'static str> {
        self.drafts[self.current].editor.error
    }
    pub fn apply(&mut self, command: Command) {
        if !self.accepts(command) {
            return;
        }
        self.focus = command;
        self.reveal = true;
        match command {
            Command::Question(index) => {
                self.current = index;
                self.scroll = 0;
                self.invalidate_geometry();
            }
            Command::Option(index) => self.drafts[self.current].answer = Answer::Option(index),
            Command::FreeText => self.drafts[self.current].answer = Answer::Text,
            Command::Skip => self.drafts[self.current].answer = Answer::Skip,
            _ => {}
        }
    }
    pub fn input(&mut self, event: Event, hits: &[Hit]) -> (bool, Option<Command>) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == KeyCode::Esc {
                    return (true, Some(Command::Close));
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
                    return (true, Some(Command::Submit));
                }
                let backwards =
                    key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
                if matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
                    || self.focus != Command::FreeText
                        && matches!(key.code, KeyCode::Up | KeyCode::Down)
                {
                    let controls = self.controls();
                    let index = controls
                        .iter()
                        .position(|command| *command == self.focus)
                        .unwrap_or(0);
                    let backwards = backwards || key.code == KeyCode::Up;
                    self.focus = controls[if backwards {
                        (index + controls.len() - 1) % controls.len()
                    } else {
                        (index + 1) % controls.len()
                    }];
                    self.reveal = true;
                    return (true, None);
                }
                if self.focus == Command::FreeText {
                    let draft = &mut self.drafts[self.current];
                    let before = draft.editor.text().to_owned();
                    let dirty = draft.editor.key(key);
                    if draft.editor.text() != before {
                        draft.answer = Answer::Text;
                    }
                    return (dirty, None);
                }
                match key.code {
                    KeyCode::Enter | KeyCode::Char(' ') => (true, Some(self.focus)),
                    KeyCode::PageUp => {
                        self.scroll = self.scroll.saturating_sub(10);
                        self.reveal = false;
                        (true, None)
                    }
                    KeyCode::PageDown => {
                        self.scroll = (self.scroll + 10).min(self.max_scroll);
                        self.reveal = false;
                        (true, None)
                    }
                    KeyCode::Home => {
                        self.scroll = 0;
                        self.reveal = false;
                        (true, None)
                    }
                    KeyCode::End => {
                        self.scroll = self.max_scroll;
                        self.reveal = false;
                        (true, None)
                    }
                    _ => (false, None),
                }
            }
            Event::Paste(text) if self.focus == Command::FreeText => {
                let draft = &mut self.drafts[self.current];
                let before = draft.editor.text().to_owned();
                let dirty = draft.editor.insert(&text);
                if draft.editor.text() != before {
                    draft.answer = Answer::Text;
                }
                (dirty, None)
            }
            Event::Mouse(mouse) => {
                let draft = &mut self.drafts[self.current];
                if (draft
                    .editor
                    .contains(Position::new(mouse.column, mouse.row))
                    || draft.editor.dragging())
                    && draft.editor.mouse(mouse)
                {
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                        self.focus = Command::FreeText;
                        draft.answer = Answer::Text;
                    }
                    return (true, None);
                }
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => {
                        let command = hits
                            .iter()
                            .rev()
                            .find(|hit| hit.area.contains(Position::new(mouse.column, mouse.row)))
                            .and_then(|hit| {
                                if let Action::Interaction(command) = hit.action {
                                    Some(command)
                                } else {
                                    None
                                }
                            });
                        (true, command)
                    }
                    MouseEventKind::ScrollUp => {
                        self.scroll = self.scroll.saturating_sub(3);
                        self.reveal = false;
                        (true, None)
                    }
                    MouseEventKind::ScrollDown => {
                        self.scroll = (self.scroll + 3).min(self.max_scroll);
                        self.reveal = false;
                        (true, None)
                    }
                    _ => (false, None),
                }
            }
            _ => (false, None),
        }
    }
    fn lines(
        &self,
        width: u16,
        i18n: &I18n,
        ascii: bool,
        colors: crate::theme::Palette,
    ) -> Vec<(Line<'static>, Option<Command>)> {
        let mut lines: Vec<(Line<'static>, Option<Command>)> = vec![];
        let mut append = |text: String, command: Option<Command>, style: Style| {
            // The protocol bounds questions/options. Layout still enforces its own capacity.
            match layout::plain(&text, width) {
                Ok(layout) => lines.extend(
                    layout
                        .lines
                        .into_iter()
                        .map(|line| (line.line.style(style), command)),
                ),
                Err(error) => lines.push((Line::raw(error), None)),
            }
        };
        append(
            self.questions[self.current].question.clone(),
            None,
            Style::default(),
        );
        append(String::new(), None, Style::default());
        for (index, option) in self.questions[self.current].options.iter().enumerate() {
            let chosen = self.drafts[self.current].answer == Answer::Option(index);
            let mark = if chosen {
                if ascii { "(*)" } else { "●" }
            } else if ascii {
                "( )"
            } else {
                "○"
            };
            let command = Command::Option(index);
            let style = if self.focus == command {
                colors.selected()
            } else {
                Style::default().fg(colors.accent)
            };
            let mut text = format!("{mark} {}", option.label);
            if let Some(description) = &option.description {
                text.push_str(&format!("\n  {description}"));
            }
            append(text, Some(command), style);
            append(String::new(), None, Style::default());
        }
        let skipped = self.drafts[self.current].answer == Answer::Skip;
        append(
            format!(
                "{} {}",
                if skipped { "[x]" } else { "[ ]" },
                i18n.text("question-skip")
            ),
            Some(Command::Skip),
            if self.focus == Command::Skip {
                colors.selected()
            } else {
                Style::default().fg(colors.accent)
            },
        );
        lines
    }
    pub fn preferred_height(&mut self, width: u16, i18n: &I18n, ascii: bool) -> u16 {
        let rows = self
            .lines(width, i18n, ascii, crate::theme::Palette::default())
            .len();
        let editor = self.drafts[self.current]
            .editor
            .preferred_height(width.saturating_sub(3), 3);
        // Border (2), question tabs (1), status/help/buttons (3), editor, content.
        rows.saturating_add(6 + usize::from(editor))
            .min(usize::from(u16::MAX)) as u16
    }
    pub fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        i18n: &I18n,
        ascii: bool,
        colors: crate::theme::Palette,
    ) -> Vec<Hit> {
        let mut hits = vec![];
        let entry_height = self.drafts[self.current].editor.preferred_height(
            area.width.saturating_sub(3),
            area.height.saturating_sub(2).clamp(1, 3),
        );
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(entry_height),
        ])
        .split(area);
        for index in 0..self.questions.len() {
            let mark = match self.answer(index) {
                None => {
                    if ascii {
                        "?"
                    } else {
                        "○"
                    }
                }
                Some(None) => "-",
                Some(Some(_)) => {
                    if ascii {
                        "*"
                    } else {
                        "✓"
                    }
                }
            };
            let rect =
                Rect::new(parts[0].x + index as u16 * 6, parts[0].y, 6, 1).intersection(parts[0]);
            frame.render_widget(
                Paragraph::new(format!(
                    "{}{} {}{}",
                    if index == self.current { "[" } else { " " },
                    index + 1,
                    mark,
                    if index == self.current { "]" } else { " " }
                ))
                .style(if self.focus == Command::Question(index) {
                    colors.selected()
                } else {
                    Style::default().fg(colors.accent)
                }),
                rect,
            );
            if !rect.is_empty() {
                hits.push(Hit {
                    area: rect,
                    action: Action::Interaction(Command::Question(index)),
                });
            }
        }
        let lines = self.lines(parts[1].width, i18n, ascii, colors);
        self.max_scroll = lines.len().saturating_sub(usize::from(parts[1].height));
        if self.reveal {
            if let Some(index) = lines
                .iter()
                .position(|(_, command)| *command == Some(self.focus))
            {
                if index < self.scroll {
                    self.scroll = index;
                } else if index >= self.scroll + usize::from(parts[1].height) {
                    self.scroll = (index + 1).saturating_sub(usize::from(parts[1].height));
                }
            }
            self.reveal = false;
        }
        self.scroll = self.scroll.min(self.max_scroll);
        for (row, (line, command)) in lines
            .into_iter()
            .skip(self.scroll)
            .take(usize::from(parts[1].height))
            .enumerate()
        {
            let rect = Rect::new(parts[1].x, parts[1].y + row as u16, parts[1].width, 1);
            frame.render_widget(Paragraph::new(line), rect);
            if let Some(command) = command {
                hits.push(Hit {
                    area: rect,
                    action: Action::Interaction(command),
                });
            }
        }
        let entry = parts[2];
        let focused = self.focus == Command::FreeText;
        let chosen = self.drafts[self.current].answer == Answer::Text;
        let prefix = if chosen {
            if ascii { "*> " } else { "●› " }
        } else if ascii {
            " > "
        } else {
            "○› "
        };
        frame.render_widget(
            Paragraph::new(prefix).style(Style::default().fg(colors.accent)),
            entry,
        );
        let text_area = Rect::new(
            entry.x + 3,
            entry.y,
            entry.width.saturating_sub(3),
            entry.height,
        )
        .intersection(entry);
        self.drafts[self.current]
            .editor
            .draw(frame, text_area, focused, colors);
        if self.drafts[self.current].editor.text().is_empty() {
            frame.render_widget(
                Paragraph::new(i18n.text("question-custom"))
                    .style(Style::default().fg(colors.subtle)),
                text_area,
            );
        }
        if !entry.is_empty() {
            hits.push(Hit {
                area: entry,
                action: Action::Interaction(Command::FreeText),
            });
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        State,
        tests::{draw, fixture},
    };
    use super::*;
    use crate::{Locale, LocalePreference, app::App};
    use crossterm::event::{KeyEvent, MouseEvent};
    use maka_client::{ClientError, RequestFailure};
    use maka_protocol::interaction::{self, InteractionAnswer};
    use serde_json::json;

    fn key(
        app: &mut App,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Option<(super::super::Ticket, Option<InteractionAnswer>)> {
        let (_, action) = app.input(Event::Key(KeyEvent::new(code, modifiers)));
        if let Some(Action::Interaction(command)) = action {
            app.interaction_request(command)
        } else {
            None
        }
    }
    fn click(app: &mut App, command: Command) {
        draw(app, 100, 30);
        let rect = app
            .hits
            .iter()
            .find(|hit| hit.action == Action::Interaction(command))
            .unwrap()
            .area;
        let (_, action) = app.input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        }));
        if let Some(Action::Interaction(command)) = action {
            assert!(app.interaction_request(command).is_none());
        }
        app.input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        }));
    }
    fn questions(app: &App) -> &Questions {
        app.interactions
            .review
            .as_ref()
            .unwrap()
            .questions
            .as_ref()
            .unwrap()
    }
    #[test]
    fn explicit_choices_custom_unicode_and_skips_share_one_guarded_submission_and_survive_later() {
        let mut app = fixture();
        let snapshot = app.chat.snapshot.as_mut().unwrap();
        let mut pending = serde_json::to_value(&snapshot.interactions.pending()[0]).unwrap();
        pending["request"] = json!({"kind":"question","toolUseId":"call","questions":[
            {"question":"Pick a destination","options":[{"label":"Alpha","description":"First choice"},{"label":"Beta"}]},
            {"question":"Explain your choice","options":[{"label":"Fast"},{"label":"Simple"}]},
            {"question":"Optional detail","options":[{"label":"Include"},{"label":"Omit"}]}
        ]});
        snapshot.interactions =
            interaction::decode_session_projection(&json!({"pending":[pending]}), "a").unwrap();
        app.open_interaction();
        let text = draw(&mut app, 100, 30);
        assert!(text.contains("Pick a destination") && text.contains("First choice"));
        let lines: Vec<_> = text.lines().collect();
        let top = lines.iter().position(|line| line.contains('┌')).unwrap();
        let bottom = lines.iter().position(|line| line.contains('└')).unwrap();
        assert!(bottom - top < 20, "short questions retain natural height");
        assert!(
            top.abs_diff(29 - bottom) <= 1,
            "question review is centered"
        );
        assert!(!app.interaction_enabled(Command::Submit));
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(
            !app.interactions.visible,
            "default Enter must not choose an answer"
        );
        app.open_interaction();
        click(&mut app, Command::Option(1));
        assert_eq!(questions(&app).answer(0), Some(Some("Beta".into())));
        click(&mut app, Command::Question(1));
        click(&mut app, Command::FreeText);
        app.input(Event::Paste("自己的回答 e\u{301}".into()));
        key(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        key(&mut app, KeyCode::Char('z'), KeyModifiers::CONTROL);
        assert_eq!(
            questions(&app).answer(1),
            Some(Some("自己的回答 e\u{301}".into()))
        );
        let before = questions(&app).drafts[1].editor.text().to_owned();
        app.input(Event::Paste("中".repeat(683))); // More than 2,048 UTF-8 bytes.
        assert_eq!(questions(&app).drafts[1].editor.text(), before);
        assert_eq!(questions(&app).error(), Some("question-too-large"));
        assert_eq!(
            app.drafts["a"].text(),
            "keep draft",
            "modal editor is not the chat composer"
        );
        key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        let pending_snapshot = app.chat.snapshot.take(); // Releasing the page observation is not discarding a draft.
        app.sync_interaction();
        assert_eq!(
            app.interactions.review.as_ref().unwrap().state,
            State::Stale
        );
        app.chat.snapshot = pending_snapshot; // Same Root/epoch and exact pending request on return.
        app.open_interaction();
        assert_eq!(questions(&app).answer(1), Some(Some(before.clone())));
        click(&mut app, Command::Question(2));
        // Keyboard activation uses the same action as a mouse choice.
        for _ in 0..12 {
            if questions(&app).focus == Command::Skip {
                break;
            }
            key(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        }
        draw(&mut app, 100, 30);
        key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            questions(&app).answers(),
            Some(vec![Some("Beta".into()), Some(before.clone()), None])
        );
        for locale in Locale::ALL {
            app.i18n.preference = LocalePreference::Explicit(locale);
            for size in [(30, 10), (80, 24), (120, 40)] {
                draw(&mut app, size.0, size.1);
            }
            assert!(app.i18n.diagnostics().is_empty());
        }
        let (ticket, answer) = key(&mut app, KeyCode::Char('s'), KeyModifiers::CONTROL).unwrap();
        assert_eq!(
            answer,
            Some(InteractionAnswer::Question {
                answers: vec![Some("Beta".into()), Some(before), None]
            })
        );
        assert!(app.interaction_request(Command::Submit).is_none());
        app.interaction_completed(
            ticket.clone(),
            Err(RequestFailure::Unknown(ClientError::Timeout)),
        );
        assert!(!app.interaction_enabled(Command::Option(0)));
        let (query, answer) = app.interaction_request(Command::Check).unwrap();
        assert_eq!(query, ticket);
        assert!(answer.is_none());
        app.interaction_completed(query, Ok(ticket.snapshot));
        assert!(app.interaction_enabled(Command::Submit));
        app.chat.snapshot.as_mut().unwrap().interactions = Default::default();
        app.sync_interaction();
        assert_eq!(
            app.interactions.review.as_ref().unwrap().state,
            State::Stale
        );
        assert!(!app.interaction_enabled(Command::Submit));
        assert!(!app.interaction_enabled(Command::FreeText));
    }
}
