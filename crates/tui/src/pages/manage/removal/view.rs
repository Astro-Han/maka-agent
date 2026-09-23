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
use crate::{
    pages::manage::view::note_lines,
    view::{button, safe},
};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    widgets::{Block, BorderType, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let busy = app.management.pending.is_some();
    let reading = app.management.removal_pending.is_some();
    let dialog = app.management.dialog.as_mut().unwrap();
    let state = dialog.removal.as_ref().unwrap();
    let width = area.width.saturating_sub(2).min(62);
    let content_width = width.saturating_sub(4);
    let title = note_lines(&safe(&dialog.target.name), content_width);
    let message = if busy {
        app.i18n.text("session-remove-wait")
    } else if let Some(error) = state.error {
        app.i18n.text(error)
    } else if reading || state.count.is_none() {
        app.i18n.text("session-remove-checking")
    } else {
        let mut note = app.i18n.text("session-remove-note");
        if let Some(count) = state.count.filter(|count| *count > 0) {
            note.push_str("\n\n");
            note.push_str(
                &app.i18n
                    .format("session-remove-subtasks", &[("count", &count.to_string())]),
            );
        }
        note
    };
    let note = note_lines(&message, content_width);
    let height = 7 + title.len() as u16 + note.len() as u16;
    if width < 36 || height > area.height.saturating_sub(2) {
        dialog.visible = false;
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    dialog.visible = true;
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(app.i18n.text("session-remove"))
        .title_alignment(ratatui::layout::Alignment::Center)
        .style(base)
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(app.theme.colors().subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    let title_rows = title.len() as u16;
    let note_rows = note.len() as u16;
    let error = state.error.is_some();
    let conflicted = state.error == Some("session-edit-conflict");
    let uncertain = state.uncertain;
    let focus = dialog.focus;
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(title),
        Rect::new(inner.x, inner.y + 1, inner.width, title_rows),
    );
    frame.render_widget(
        Paragraph::new(note).style(Style::default().fg(if error {
            app.theme.colors().warning
        } else {
            app.theme.colors().muted
        })),
        Rect::new(inner.x, inner.y + 2 + title_rows, inner.width, note_rows),
    );
    let primary = if error {
        Command::RemovalQuery
    } else {
        Command::Save
    };
    let label = app.i18n.text(if error {
        if uncertain {
            "session-remove-query"
        } else {
            "session-remove-retry"
        }
    } else {
        "session-remove-confirm"
    });
    let cancel = app.i18n.text(if busy || error {
        "session-remove-close"
    } else {
        "session-cancel"
    });
    let primary_width = if conflicted {
        0
    } else {
        (label.width() as u16 + 2).min(inner.width / 2)
    };
    let cancel_width = (cancel.width() as u16 + 2).min(inner.width / 2);
    let y = inner.bottom() - 1;
    button(
        frame,
        app,
        Rect::new(
            inner.right() - primary_width - cancel_width - u16::from(primary_width > 0),
            y,
            cancel_width,
            1,
        ),
        &cancel,
        Action::Manage(Command::Close),
        focus == 0,
    );
    if primary_width > 0 {
        button(
            frame,
            app,
            Rect::new(inner.right() - primary_width, y, primary_width, 1),
            &label,
            Action::Manage(primary),
            focus == 1,
        );
    }
}
