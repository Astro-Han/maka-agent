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

use super::{Command, State, summary};
use crate::{
    app::{Action, App, Hit},
    pages::chat::layout,
    view::safe,
};
use maka_protocol::interaction::InteractionOutcome;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Block, Paragraph},
};

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    app.hits.clear();
    let Some(review) = &mut app.interactions.review else {
        return;
    };
    let width = area.width.saturating_sub(2).min(100);
    let mut buttons = vec![];
    let (mut x, mut y) = (0u16, 0u16);
    for command in review.commands() {
        let label = app
            .i18n
            .text(if command == Command::Details && review.details {
                "interaction-hide-details"
            } else {
                command.label()
            });
        let cells = (unicode_width::UnicodeWidthStr::width(label.as_str()) + 2)
            .min(usize::from(width.saturating_sub(2))) as u16;
        if x + cells > width.saturating_sub(2) {
            x = 0;
            y += 1;
        }
        buttons.push((command, label, Rect::new(x, y, cells, 1)));
        x += cells;
    }
    let button_rows = y + 1;
    let editing_questions = review.state == State::Ready && review.questions.is_some();
    let editing_form = review.state == State::Ready && review.form.is_some();
    let content = (!editing_questions && !editing_form)
        .then(|| layout::plain(&summary::text(review, &app.i18n), width.saturating_sub(2)));
    let height = area.height.saturating_sub(2);
    let height = if review.state == State::Ready
        && let Some(questions) = &mut review.questions
    {
        questions
            .preferred_height(width.saturating_sub(2), &app.i18n, app.chrome.ascii)
            .max(8)
            .min(height)
    } else if review.state == State::Ready
        && let Some(form) = &mut review.form
    {
        form.preferred_height(
            width.saturating_sub(2),
            &app.i18n,
            app.chrome.ascii,
            button_rows,
        )
        .max(8)
        .min(height)
    } else {
        content
            .as_ref()
            .and_then(|content| content.as_ref().ok())
            .map(|layout| {
                layout
                    .lines
                    .len()
                    .saturating_add(4 + usize::from(button_rows))
            })
            .unwrap_or(8)
            .max(8)
            .min(usize::from(height)) as u16
    };
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    let block = Block::bordered()
        .title(app.i18n.text("interaction-title"))
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().warning));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let body = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        inner.height.saturating_sub(button_rows + 2),
    );
    let state = match review.state {
        State::Ready => "interaction-ready",
        State::Sending => "interaction-sending",
        State::Unknown => "interaction-unknown",
        State::Checking => "interaction-checking",
        State::Stale => "interaction-stale",
        State::Resolved => match review
            .outcome
            .as_ref()
            .and_then(|outcome| outcome.outcome())
        {
            Some(InteractionOutcome::Closure { .. }) => "interaction-closed",
            _ => "interaction-resolved",
        },
    };
    if editing_questions {
        app.hits.extend(review.questions.as_mut().unwrap().draw(
            frame,
            body,
            &app.i18n,
            app.chrome.ascii,
            app.theme.colors(),
        ));
    } else if editing_form {
        app.hits.extend(review.form.as_mut().unwrap().draw(
            frame,
            body,
            &app.i18n,
            app.chrome.ascii,
            app.theme.colors(),
        ));
    } else {
        match content.expect("non-editing review has display content") {
            Ok(layout) => {
                review.max_scroll = layout.lines.len().saturating_sub(usize::from(body.height));
                review.scroll = review.scroll.min(review.max_scroll);
                for (row, line) in layout
                    .lines
                    .into_iter()
                    .skip(review.scroll)
                    .take(usize::from(body.height))
                    .enumerate()
                {
                    frame.render_widget(
                        Paragraph::new(line.line),
                        Rect::new(body.x, body.y + row as u16, body.width, 1),
                    );
                }
            }
            Err(error) => frame.render_widget(Paragraph::new(safe(error)), body),
        }
    }
    let status = if editing_questions {
        let questions = review.questions.as_ref().unwrap();
        questions
            .error()
            .map(|key| app.i18n.text(key))
            .unwrap_or_else(|| {
                app.i18n.format(
                    "question-progress",
                    &[
                        ("value", &questions.answered().to_string()),
                        ("count", &questions.len().to_string()),
                    ],
                )
            })
    } else if editing_form {
        review.form.as_ref().unwrap().status(&app.i18n)
    } else {
        app.i18n.text(state)
    };
    let status_y = body.bottom();
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(app.theme.colors().warning)),
        Rect::new(inner.x, status_y, inner.width, 1).intersection(inner),
    );
    frame.render_widget(
        Paragraph::new(app.i18n.text(if editing_form {
            "form-help"
        } else if editing_questions {
            "question-help"
        } else {
            "interaction-help"
        }))
        .style(Style::default().fg(app.theme.colors().subtle)),
        Rect::new(inner.x, status_y + 1, inner.width, 1).intersection(inner),
    );
    for (index, (command, label, mut rect)) in buttons.into_iter().enumerate() {
        rect.x += inner.x;
        rect.y += status_y + 2;
        rect = rect.intersection(inner);
        let enabled = (matches!(command, Command::Close | Command::Details)
            || matches!(&app.connection, crate::app::ConnectionState::Connected {root_id,..} if *root_id == review.ticket.root))
            && (command != Command::Submit
                || review
                    .questions
                    .as_ref()
                    .is_some_and(|questions| questions.answers().is_some()))
            && (command != Command::FormSubmit
                || review
                    .form
                    .as_ref()
                    .is_some_and(|form| form.result().is_some()));
        let focused = if editing_questions {
            review.questions.as_ref().unwrap().focus == command
        } else if editing_form {
            review.form.as_ref().unwrap().focus == command
        } else {
            index == review.selected
        };
        let style = if !enabled {
            Style::default().fg(app.theme.colors().subtle)
        } else if focused {
            app.theme.colors().selected()
        } else {
            Style::default().fg(app.theme.colors().accent)
        };
        frame.render_widget(Paragraph::new(format!(" {label} ")).style(style), rect);
        if enabled && !rect.is_empty() {
            app.hits.push(Hit {
                area: rect,
                action: Action::Interaction(command),
            });
        }
    }
}
