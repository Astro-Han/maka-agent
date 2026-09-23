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
use crate::view::{button, safe};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    app.hits.clear();
    if area.width < 42 || area.height < 18 {
        app.skills.invalidate_geometry();
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let rows = app.skill_rows();
    let target = app.skills.dialog.as_ref().unwrap().target.clone();
    let picked = app.picked_skills(&target).to_vec();
    let width = area.width.saturating_sub(2).min(76);
    let dialog = app.skills.dialog.as_ref().unwrap();
    let desired_height = if dialog.loading || dialog.requested {
        29
    } else {
        (rows.len() * 2 + 10).clamp(16, 29) as u16
    };
    let height = area.height.saturating_sub(2).min(desired_height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    let colors = app.theme.colors();
    let block = Block::bordered()
        .border_type(if app.chrome.ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        })
        .title(app.i18n.text("skills-title"))
        .title_alignment(ratatui::layout::Alignment::Center)
        .style(base)
        .border_style(Style::default().fg(colors.subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    frame.render_widget(block, popup);
    let capacity = usize::from(inner.height.saturating_sub(8) / 2).max(1);
    let list = Rect::new(inner.x, inner.y + 3, inner.width, (capacity * 2) as u16);
    let d = app.skills.dialog.as_mut().unwrap();
    d.visible = true;
    d.area = Some(list);
    d.selected = d.selected.min(rows.len().saturating_sub(1));
    d.top = d
        .top
        .min(d.selected)
        .max(d.selected.saturating_sub(capacity - 1))
        .min(rows.len().saturating_sub(capacity));
    let (top, selected, focus, selected_only, loading, error) = (
        d.top,
        d.selected,
        d.focus,
        d.selected_only,
        d.loading || d.requested,
        d.error,
    );
    let scrollable = rows.len() > capacity;
    for (index, row) in rows.iter().enumerate().skip(top).take(capacity) {
        let rect = Rect::new(
            list.x,
            list.y + ((index - top) * 2) as u16,
            list.width.saturating_sub(if scrollable { 2 } else { 0 }),
            1,
        );
        let checked = picked.iter().any(|p| p.id == row.id);
        let label = format!(
            "{} {}",
            if checked {
                app.chrome.symbol("✓", "x")
            } else {
                " "
            },
            safe(&row.name)
        );
        crate::view::list_item(
            frame,
            app,
            rect,
            &label,
            Action::Skills(Command::Toggle(index)),
            focus == 0 && selected == index,
        );
    }
    if scrollable {
        let widget = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(app.chrome.symbol("│", "|")))
            .thumb_symbol(app.chrome.symbol("┃", "#"))
            .track_style(Style::default().fg(colors.subtle))
            .thumb_style(Style::default().fg(colors.accent));
        let mut state = ScrollbarState::new(rows.len() - capacity + 1)
            .position(top)
            .viewport_content_length(capacity);
        frame.render_stateful_widget(widget, list, &mut state);
    }
    let selected_label = format!("{} · {}", app.i18n.text("skills-selected"), picked.len());
    button(
        frame,
        app,
        Rect::new(inner.x, inner.y, selected_label.width() as u16 + 2, 1),
        &selected_label,
        Action::Skills(Command::Selected),
        focus == 1 || selected_only,
    );
    for (n, command, label) in [
        (2, Command::Refresh, app.chrome.symbol("⟳", "r")),
        (3, Command::Previous, app.chrome.symbol("‹", "<")),
        (4, Command::Next, app.chrome.symbol("›", ">")),
    ] {
        button(
            frame,
            app,
            Rect::new(inner.right() - ((5 - n) * 4) as u16, inner.y, 3, 1),
            label,
            Action::Skills(command),
            focus == n,
        );
    }
    let note = if let Some(error) = error {
        app.i18n.text(error)
    } else if loading {
        app.i18n.text("skills-loading")
    } else if rows.is_empty() {
        app.i18n.text("skills-empty")
    } else {
        safe(&rows[selected].description)
    };
    frame.render_widget(
        Paragraph::new(note)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(if error.is_some() {
                colors.warning
            } else {
                colors.subtle
            })),
        Rect::new(inner.x, inner.bottom() - 4, inner.width, 2),
    );
    let label = app.i18n.text("session-remove-close");
    let width = (label.width() as u16 + 4).min(inner.width);
    button(
        frame,
        app,
        Rect::new(
            inner.x + (inner.width - width) / 2,
            inner.bottom() - 1,
            width,
            1,
        ),
        &label,
        Action::Skills(Command::Close),
        focus == 5,
    );
}

pub fn chips(frame: &mut Frame<'_>, app: &mut App, area: Rect, session: &str) {
    let names = app
        .skills
        .saved
        .get(session)
        .into_iter()
        .flatten()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    let label = if app.stop_target().is_some() {
        format!(
            "{} {} · {}",
            app.chrome.symbol("✧", "*"),
            safe(&names),
            app.i18n.text("skills-idle")
        )
    } else {
        format!("{} {}", app.chrome.symbol("✧", "*"), safe(&names))
    };
    crate::view::list_item(
        frame,
        app,
        area,
        &label,
        Action::Skills(Command::Open),
        false,
    );
}
