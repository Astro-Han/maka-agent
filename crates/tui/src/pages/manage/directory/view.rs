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
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let reference = app.directory_reference_active();
    let references = app
        .directory_reference_target()
        .map(|t| app.reference_items(t).to_vec())
        .unwrap_or_default();
    let reference_rows = references.len() as u16;
    let full = app
        .directory_reference_target()
        .is_some_and(|t| app.reference_count(t) >= 4);
    let busy = app.management.pending.is_some();
    let dialog = app.management.dialog.as_mut().expect("directory dialog");
    let browser = dialog.browser.as_mut().expect("directory browser");
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
        .min((browser.rows.len() as u16 + 10 + reference_rows).clamp(15 + reference_rows, 29));
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
        .title(app.i18n.text(if reference {
            "references-title"
        } else {
            "directory-title"
        }))
        .title_alignment(ratatui::layout::Alignment::Center)
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    dialog.visible = true;
    let path = browser.location.as_ref().map_or_else(
        || app.i18n.text("directory-roots"),
        |location| {
            std::iter::once(location.label.as_str())
                .chain(location.segments.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" / ")
        },
    );
    // Labels are presentation only, never joined into a Host filesystem path.
    let path = safe(&path);
    let mut tail = String::new();
    if path.width() > inner.width as usize {
        for grapheme in path.graphemes(true).rev() {
            if tail.width() + grapheme.width() + 1 > inner.width as usize {
                break;
            }
            tail.insert_str(0, grapheme);
        }
        tail.insert(0, if app.chrome.ascii { '.' } else { '…' });
    } else {
        tail = path;
    }
    frame.render_widget(
        Paragraph::new(tail),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let list = Rect::new(
        inner.x,
        inner.y + 3,
        inner.width,
        inner.height.saturating_sub(8 + reference_rows),
    );
    let offset = (browser.selected + 1).saturating_sub(list.height as usize);
    let enabled = browser.ready() && !dialog.blocked && !busy;
    for (index, row) in browser
        .rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(list.height as usize)
    {
        let rect = Rect::new(list.x, list.y + (index - offset) as u16, list.width, 1);
        let command = Manage::Directory(Command::Open(index));
        let active = browser.focus == 0 && browser.selected == index
            || browser.hovered.as_ref() == Some(&command);
        frame.render_widget(
            Paragraph::new(format!(
                "{} {}",
                if app.chrome.ascii { ">" } else { "›" },
                safe(&row.name)
            ))
            .style(if !enabled {
                Style::default().fg(app.theme.colors().subtle)
            } else if active {
                app.theme.colors().selected()
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
    if browser.rows.is_empty() && !browser.error {
        let key = if browser.loading || browser.requested {
            "projects-loading"
        } else if browser.location.is_some() {
            "directory-empty"
        } else {
            "directory-no-roots"
        };
        frame.render_widget(
            Paragraph::new(app.i18n.text(key))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(app.theme.colors().subtle)),
            list,
        );
    }
    let key = if busy {
        Some("session-saving")
    } else if dialog.error.is_some() {
        dialog.error
    } else if browser.error {
        Some("directory-failed")
    } else if full {
        Some("references-limit")
    } else {
        let command = browser.hovered.clone().unwrap_or_else(|| focused(browser));
        match command {
            Manage::Directory(
                Command::Parent | Command::Refresh | Command::Previous | Command::Next,
            ) => Some(command.label()),
            _ => None,
        }
    };
    if let Some(key) = key {
        let lines = super::super::view::note_lines(&app.i18n.text(key), inner.width);
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().fg(
                if dialog.error.is_some() || browser.error {
                    app.theme.colors().warning
                } else {
                    app.theme.colors().subtle
                },
            )),
            Rect::new(inner.x, inner.bottom() - 5, inner.width, 3),
        );
    }
    let focus = browser.focus;
    let hovered = browser.hovered.clone();
    let button_width = |text: &str| (text.width() as u16 + 2).min(inner.width / 2);
    let register = app.i18n.text(if reference {
        "references-select"
    } else {
        "directory-register"
    });
    let cancel = app.i18n.text("session-cancel");
    let path = app.i18n.text("directory-path");
    let save_width = button_width(&register);
    let cancel_width = button_width(&cancel);
    let save = Rect::new(
        inner.right() - save_width,
        inner.bottom() - 1,
        save_width,
        1,
    );
    let cancel_rect = Rect::new(save.x - cancel_width - 1, save.y, cancel_width, 1);
    let path_rect = Rect::new(
        inner.x,
        save.y,
        button_width(&path).min(cancel_rect.x.saturating_sub(inner.x + 1)),
        1,
    );
    for (rect, text, command, index) in [
        (path_rect, path, Manage::Directory(Command::Path), 1),
        (cancel_rect, cancel, Manage::Close, 6),
        (save, register, Manage::Save, 7),
    ] {
        if reference && index == 1 {
            continue;
        }
        let active = focus == index || hovered.as_ref() == Some(&command);
        button(frame, app, rect, &text, Action::Manage(command), active);
    }
    for (index, item) in references.iter().enumerate() {
        let command = Manage::Directory(Command::RemoveReference(index));
        let active = focus == index + 8 || hovered.as_ref() == Some(&command);
        crate::view::list_item(
            frame,
            app,
            Rect::new(
                inner.x,
                inner.bottom() - 5 - reference_rows + index as u16,
                inner.width,
                1,
            ),
            &format!("{}  {}", app.chrome.symbol("×", "x"), safe(&item.path)),
            Action::Manage(command),
            active,
        );
    }
    for (index, command, icon, ascii) in [
        (2, Command::Parent, "↑", "^"),
        (3, Command::Refresh, "⟳", "R"),
        (4, Command::Previous, "‹", "<"),
        (5, Command::Next, "›", ">"),
    ] {
        let command = Manage::Directory(command);
        let active = focus == index || hovered.as_ref() == Some(&command);
        button(
            frame,
            app,
            Rect::new(inner.x + ((index - 2) * 4) as u16, inner.y + 1, 3, 1),
            if app.chrome.ascii { ascii } else { icon },
            Action::Manage(command),
            active,
        );
    }
}
