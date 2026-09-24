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
    app::{Action, App, Focus, Hit},
    pages::chat::layout,
};
use maka_plugins::terminal_ui::page::Control;
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    app.extensions.invalidate_geometry();
    let mut area = area.inner(Margin::new(2, 1));
    if area.is_empty() {
        return;
    }
    let colors = app.theme.colors();
    if !app.extensions.busy && area.height >= 5 {
        let commands = app.extensions.contextual_actions();
        // Keep contextual recovery actions on one line whenever they fit.
        let buttons: Vec<_> = commands
            .into_iter()
            .map(|command| {
                let label = app.i18n.text(command.label());
                let width = (unicode_width::UnicodeWidthStr::width(label.as_str()) as u16 + 4)
                    .min(area.width);
                (command, label, width)
            })
            .collect();
        let horizontal = buttons.iter().map(|(_, _, width)| width).sum::<u16>()
            + 2 * buttons.len().saturating_sub(1) as u16
            <= area.width;
        let mut x = area.x;
        let mut y = area.y;
        for (command, label, width) in &buttons {
            let selected = app.focus == Focus::Page
                && app.page_actions().get(app.selected_control)
                    == Some(&Action::Extension(command.clone()));
            crate::view::button(
                frame,
                app,
                Rect::new(x, y, *width, 1),
                label,
                Action::Extension(command.clone()),
                selected,
            );
            if horizontal {
                x += width + 2;
            } else {
                y += 2;
            }
        }
        let rows = if buttons.is_empty() {
            0
        } else if horizontal {
            2
        } else {
            2 * buttons.len() as u16
        };
        area.y += rows;
        area.height = area.height.saturating_sub(rows);
    }
    if app.extensions.review.is_some() {
        super::drafts::draw(frame, app, area);
        return;
    }
    let consent = app.extensions.consent_visible();
    let locale = app.i18n.locale().id();
    let form_width = area.width.saturating_sub(1).min(52);
    let mut lines = Vec::new();
    let mut controls = Vec::new();
    let state = &app.extensions;
    if let Some(message) = &state.message {
        let color = if matches!(message, super::Message::Local("extensions-draft-ready")) {
            colors.muted
        } else {
            colors.warning
        };
        let message = match message {
            super::Message::Local(key) => app.i18n.text(key),
            super::Message::Remote(text) => text.resolve(locale).into(),
        };
        for line in layout::plain(&message, area.width.saturating_sub(1))
            .unwrap()
            .lines
        {
            lines.push(line.line.style(Style::default().fg(color)));
        }
        lines.push(Line::default());
    }
    if state.busy {
        lines.push(Line::styled(
            app.i18n.text("extensions-loading"),
            Style::default().fg(colors.muted),
        ));
        lines.push(Line::default());
    }
    if let Some(page) = &state.page {
        lines.push(Line::styled(
            page.title.resolve(locale).to_owned(),
            Style::default()
                .fg(colors.accent)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::default());
        if !page.body.is_empty() {
            lines.extend(
                layout::plain(&page.body, area.width.saturating_sub(1))
                    .unwrap()
                    .lines
                    .into_iter()
                    .map(|line| line.line),
            );
            lines.push(Line::default());
        }
        for (index, row) in page.rows.iter().enumerate() {
            controls.push((lines.len(), 2, Command::Row(index)));
            lines.push(Line::raw(row.title.resolve(locale).to_owned()));
            lines.push(Line::styled(
                fit(&row.description, usize::from(area.width.saturating_sub(1))),
                Style::default().fg(colors.muted),
            ));
            lines.push(Line::default());
        }
        for (index, field) in page.fields.iter().enumerate() {
            let start = lines.len();
            match &field.control {
                Control::Toggle { .. } => {
                    let checked = state
                        .drafts
                        .get(&field.id)
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    let mark = if checked {
                        app.chrome.symbol("━●", "[x]")
                    } else {
                        app.chrome.symbol("○─", "[ ]")
                    };
                    let label = fit(
                        field.label.resolve(locale),
                        usize::from(form_width.saturating_sub(6)),
                    );
                    let gap = usize::from(form_width).saturating_sub(
                        unicode_width::UnicodeWidthStr::width(label.as_str())
                            + unicode_width::UnicodeWidthStr::width(mark),
                    );
                    lines.push(Line::from(vec![
                        Span::raw(label),
                        Span::raw(" ".repeat(gap)),
                        Span::styled(
                            mark,
                            Style::default().fg(if checked { colors.accent } else { colors.muted }),
                        ),
                    ]));
                }
                Control::Text { multiline, .. } => {
                    lines.push(Line::styled(
                        fit(field.label.resolve(locale), usize::from(form_width)),
                        Style::default().fg(colors.muted),
                    ));
                    lines.push(Line::default());
                    if *multiline {
                        lines.extend([Line::default(), Line::default()]);
                    }
                }
            }
            controls.push((start, lines.len() - start, Command::Field(index)));
            lines.push(Line::default());
        }
        for index in 0..page.actions.len() {
            controls.push((lines.len(), 1, Command::Submit(index)));
            lines.push(Line::default());
            lines.push(Line::default());
        }
    } else {
        for (index, view) in state.directory.iter().enumerate() {
            controls.push((lines.len(), 2, Command::Choose(index)));
            lines.push(Line::raw(view.descriptor.title.resolve(locale).to_owned()));
            let detail = if view.descriptor.context == maka_plugins::terminal_ui::Context::Session
                && state.session.is_none()
            {
                app.i18n.text("extensions-needs-session")
            } else {
                view.package_id.clone()
            };
            lines.push(Line::styled(detail, Style::default().fg(colors.muted)));
            lines.push(Line::default());
        }
        if state.directory.is_empty() && !state.busy && state.message.is_none() {
            lines.push(Line::styled(
                app.i18n.text("extensions-empty"),
                Style::default().fg(colors.muted),
            ));
        }
    }
    let state = &mut app.extensions;
    if state.reveal {
        if let Some((start, height, _)) = controls.get(state.selected) {
            if *start < state.top {
                state.top = *start;
            } else if start + height > state.top + usize::from(area.height) {
                state.top = (start + height).saturating_sub(usize::from(area.height));
            }
        }
        state.reveal = false;
    }
    state.top = state
        .top
        .min(lines.len().saturating_sub(usize::from(area.height)));
    state.area = Some(area);
    let top = state.top;
    let total = lines.len();
    for (index, (start, height, command)) in controls.iter().enumerate() {
        let action = Action::Extension(command.clone());
        let enabled = app.enabled(&action);
        let focused = app.palette.is_none()
            && (app.focus == Focus::List && app.extensions.selected == index
                || app.hover.as_ref() == Some(&action));
        if focused || !enabled {
            let style = Style::default().fg(if !enabled {
                colors.muted
            } else {
                colors.accent
            });
            for line in lines.iter_mut().skip(*start).take(*height) {
                *line = line.clone().style(style);
            }
        }
    }
    frame.render_widget(Paragraph::new(lines).scroll((top as u16, 0)), area);
    for (index, (start, height, command)) in controls.iter().enumerate() {
        let end = (start + height).min(top + usize::from(area.height));
        if end <= top || *start >= top + usize::from(area.height) {
            continue;
        }
        let y = start.saturating_sub(top);
        let rect = Rect::new(
            area.x,
            area.y + y as u16,
            if matches!(command, Command::Field(_) | Command::Submit(_)) {
                form_width
            } else {
                area.width.saturating_sub(1)
            },
            (end - (*start).max(top)) as u16,
        );
        if let Command::Submit(action_index) = command {
            let action = &app.extensions.page.as_ref().unwrap().actions[*action_index];
            let label =
                if app.extensions.applied.as_deref() == Some(&action.id) && !app.extensions.busy {
                    format!(
                        "{} {}",
                        app.chrome.symbol("✓", "+"),
                        action.label.resolve(locale)
                    )
                } else {
                    action.label.resolve(locale).to_owned()
                };
            crate::view::button(
                frame,
                app,
                rect,
                &label,
                Action::Extension(command.clone()),
                app.focus == Focus::List && app.extensions.selected == index,
            );
        } else {
            app.hits.push(Hit {
                area: rect,
                action: Action::Extension(command.clone()),
            });
        }
        if let Command::Field(field_index) = command
            && let Some(field) = app
                .extensions
                .page
                .as_ref()
                .and_then(|page| page.fields.get(*field_index))
            && let Some(editor) = app.extensions.editors.get_mut(&field.id)
            && *start >= top
            && *start + height <= top + usize::from(area.height)
        {
            editor.draw(
                frame,
                Rect::new(
                    rect.x,
                    rect.y + 1,
                    rect.width,
                    rect.height.saturating_sub(1),
                ),
                app.focus == Focus::List
                    && index == app.extensions.selected
                    && app.palette.is_none()
                    && !consent
                    && !app.extensions.busy
                    && !app.extensions.blocked
                    && field.enabled,
                colors,
            );
        }
    }
    if total > usize::from(area.height) {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .track_symbol(Some(app.chrome.symbol("│", "|")))
                .thumb_symbol(app.chrome.symbol("┃", "#"))
                .track_style(Style::default().fg(colors.subtle))
                .thumb_style(Style::default().fg(colors.muted))
                .begin_symbol(None)
                .end_symbol(None),
            area,
            &mut ScrollbarState::new(total.saturating_sub(usize::from(area.height))).position(top),
        );
    }
}

fn fit(text: &str, width: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    if text.width() <= width {
        return text.into();
    }
    let mut result = String::new();
    let mut cells = 0;
    for glyph in text.graphemes(true) {
        if cells + glyph.width() >= width {
            break;
        }
        result.push_str(glyph);
        cells += glyph.width();
    }
    if width > 0 {
        result.push('…');
    }
    result
}
