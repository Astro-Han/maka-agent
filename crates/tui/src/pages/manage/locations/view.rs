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

use super::{Command, Manage, focused};
use crate::{
    app::{Action, App, Hit},
    view::{button, safe},
};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    text::Line,
    widgets::{Block, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let dialog = app.management.dialog.as_mut().expect("locations dialog");
    let locations = dialog.locations.as_mut().expect("locations reader");
    if area.width < 42 || area.height < 13 {
        dialog.visible = false;
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let width = area.width.saturating_sub(2).min(84);
    let content_width = width.saturating_sub(4);
    let mut lines = vec![];
    let error = dialog.error.or(locations.error);
    if let Some(error) = error {
        lines = super::super::view::note_lines(&app.i18n.text(error), content_width);
    } else if !locations.ready() {
        lines.push(Line::from(app.i18n.text("projects-loading")));
    } else if locations.rows.is_empty() {
        lines.push(Line::from(app.i18n.text("project-locations-empty")));
    } else {
        for (index, location) in &locations.rows {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            let tags = [
                (
                    Some(*index) == locations.preferred,
                    "project-location-preferred",
                ),
                (location.is_worktree, "project-location-worktree"),
            ]
            .into_iter()
            .filter(|(yes, _)| *yes)
            .map(|(_, key)| app.i18n.text(key))
            .collect::<Vec<_>>();
            if !tags.is_empty() {
                lines.push(Line::styled(
                    tags.join(" · "),
                    Style::default().fg(app.theme.colors().subtle),
                ));
            }
            lines.extend(super::super::view::note_lines(
                &safe(&location.path),
                content_width,
            ));
        }
    }
    let height = area
        .height
        .saturating_sub(2)
        .min((lines.len().min(18) as u16 + 7).max(11));
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .border_type(if app.chrome.ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        })
        .title(app.i18n.text("project-locations"))
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    dialog.visible = true;
    frame.render_widget(
        Paragraph::new(safe(
            locations.name.as_deref().unwrap_or(&dialog.target.name),
        )),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let content = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(5),
    );
    locations.max_offset = lines.len().saturating_sub(content.height as usize);
    locations.offset = locations.offset.min(locations.max_offset);
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(locations.offset)
                .take(content.height as usize)
                .collect::<Vec<_>>(),
        )
        .style(if error.is_some() {
            Style::default().fg(app.theme.colors().warning)
        } else {
            Style::default()
        }),
        content,
    );
    if locations.ready() && !dialog.blocked {
        app.hits.push(Hit {
            area: content,
            action: Action::Manage(Manage::Locations(Command::Scroll(true))),
        });
    }
    let hovered = locations.hovered.clone();
    let focus = locations.focus;
    let tooltip = hovered.clone().unwrap_or_else(|| focused(locations));
    if matches!(
        tooltip,
        Manage::Locations(Command::Refresh | Command::Previous | Command::Next)
    ) {
        frame.render_widget(
            Paragraph::new(app.i18n.text(tooltip.label()))
                .style(Style::default().fg(app.theme.colors().subtle)),
            Rect::new(inner.x, inner.bottom() - 2, inner.width, 1),
        );
    }
    // Scroll position is shown only when content exceeds the viewport.
    if locations.max_offset > 0 {
        let marker = if locations.offset == 0 {
            "↓"
        } else if locations.offset == locations.max_offset {
            "↑"
        } else {
            "↕"
        };
        frame.render_widget(
            Paragraph::new(if app.chrome.ascii { ":" } else { marker })
                .style(Style::default().fg(app.theme.colors().subtle)),
            Rect::new(inner.right() - 1, content.bottom(), 1, 1),
        );
    }
    let close = app.i18n.text("project-locations-close");
    let close_width = (close.width() as u16 + 2).min(inner.width);
    button(
        frame,
        app,
        Rect::new(
            inner.right() - close_width,
            inner.bottom() - 1,
            close_width,
            1,
        ),
        &close,
        Action::Manage(Manage::Close),
        focus == 4 || hovered.as_ref() == Some(&Manage::Close),
    );
    for (index, command, icon, ascii) in [
        (1, Command::Refresh, "⟳", "R"),
        (2, Command::Previous, "‹", "<"),
        (3, Command::Next, "›", ">"),
    ] {
        let command = Manage::Locations(command);
        let active = focus == index || hovered.as_ref() == Some(&command);
        button(
            frame,
            app,
            Rect::new(inner.x + ((index - 1) * 4) as u16, inner.bottom() - 1, 3, 1),
            if app.chrome.ascii { ascii } else { icon },
            Action::Manage(command),
            active,
        );
    }
}
