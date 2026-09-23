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
    widgets::{Block, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let busy = app.management.pending.is_some();
    let dialog = app
        .management
        .dialog
        .as_mut()
        .expect("project chooser dialog");
    let chooser = dialog.chooser.as_mut().expect("project chooser");
    if area.width < 42 || area.height < 17 {
        dialog.visible = false;
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let width = area.width.saturating_sub(2).min(72);
    let height = area
        .height
        .saturating_sub(2)
        .min((chooser.catalog.items.len() as u16 + 10).clamp(15, 25));
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
        .title(app.i18n.text("session-project-change"))
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    dialog.visible = true;
    frame.render_widget(
        Paragraph::new(safe(&dialog.target.name)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list = Rect::new(
        inner.x,
        inner.y + 3,
        inner.width,
        inner.height.saturating_sub(8),
    );
    let selected = chooser
        .catalog
        .items
        .iter()
        .position(|item| Some(&item.id) == chooser.catalog.selected.as_ref());
    let offset = selected.map_or(0, |index| (index + 1).saturating_sub(list.height as usize));
    // Selection is local intent; only submission requires a fresh catalog.
    let enabled = !dialog.blocked && !busy;
    for (index, item) in chooser
        .catalog
        .items
        .iter()
        .enumerate()
        .skip(offset)
        .take(list.height as usize)
    {
        let rect = Rect::new(list.x, list.y + (index - offset) as u16, list.width, 1);
        let command = Manage::ChooseProject(Command::Select(item.id.clone()));
        let active = selected == Some(index) || chooser.hovered.as_ref() == Some(&command);
        let marker = if selected == Some(index) {
            if app.chrome.ascii { ">" } else { "›" }
        } else {
            " "
        };
        let mut label = format!("{marker} {}", safe(&item.name));
        if item.archived {
            label.push_str(&format!(" · {}", app.i18n.text("session-archived")));
        } else if !item.available {
            label.push_str(&format!(" · {}", app.i18n.text("project-unavailable")));
        }
        frame.render_widget(
            Paragraph::new(label).style(if !enabled {
                Style::default().fg(app.theme.colors().subtle)
            } else if active {
                app.theme.colors().selected()
            } else if !item.usable() {
                Style::default().fg(app.theme.colors().subtle)
            } else {
                Style::default()
            }),
            rect,
        );
        if enabled {
            app.hits.push(Hit {
                area: rect,
                action: Action::Manage(command),
            });
        }
    }
    if chooser.catalog.items.is_empty() && !chooser.catalog.error {
        let key = if !chooser.catalog.ready() {
            "projects-loading"
        } else if chooser.catalog.can_next() || chooser.catalog.can_previous() {
            "projects-page-empty"
        } else {
            "projects-empty"
        };
        frame.render_widget(
            Paragraph::new(app.i18n.text(key))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(app.theme.colors().subtle)),
            list,
        );
    }
    let key = if busy {
        "session-saving"
    } else if let Some(error) = dialog.error {
        error
    } else if chooser.catalog.error {
        "projects-failed"
    } else {
        let command = chooser.hovered.clone().unwrap_or_else(|| focused(chooser));
        match command {
            Manage::ChooseProject(Command::Refresh | Command::Previous | Command::Next) => {
                command.label()
            }
            _ => "session-project-note",
        }
    };
    frame.render_widget(
        Paragraph::new(super::super::view::note_lines(
            &app.i18n.text(key),
            inner.width,
        ))
        .style(
            Style::default().fg(if dialog.error.is_some() || chooser.catalog.error {
                app.theme.colors().warning
            } else {
                app.theme.colors().subtle
            }),
        ),
        Rect::new(inner.x, inner.bottom() - 5, inner.width, 3),
    );
    let focus = chooser.focus;
    let hovered = chooser.hovered.clone();
    let save = app.i18n.text("session-project-apply");
    let cancel = app.i18n.text("session-cancel");
    let save_width = (save.width() as u16 + 2).min(inner.width / 2);
    let cancel_width = (cancel.width() as u16 + 2).min(inner.width / 2);
    let save_rect = Rect::new(
        inner.right() - save_width,
        inner.bottom() - 1,
        save_width,
        1,
    );
    let cancel_rect = Rect::new(save_rect.x - cancel_width - 1, save_rect.y, cancel_width, 1);
    for (rect, text, command, index) in [
        (cancel_rect, cancel, Manage::Close, 4),
        (save_rect, save, Manage::Save, 5),
    ] {
        let active = focus == index || hovered.as_ref() == Some(&command);
        button(frame, app, rect, &text, Action::Manage(command), active);
    }
    for (index, command, icon, ascii) in [
        (1, Command::Refresh, "⟳", "R"),
        (2, Command::Previous, "‹", "<"),
        (3, Command::Next, "›", ">"),
    ] {
        let command = Manage::ChooseProject(command);
        let active = focus == index || hovered.as_ref() == Some(&command);
        button(
            frame,
            app,
            Rect::new(inner.x + ((index - 1) * 4) as u16, inner.y + 1, 3, 1),
            if app.chrome.ascii { ascii } else { icon },
            Action::Manage(command),
            active,
        );
    }
}
