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

use super::{Command, address};
use crate::{
    app::{Action, App},
    pages::manage::{Entity, view::note_lines},
    view::{button, safe},
};
use maka_protocol::configuration::CredentialState;
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    widgets::{Block, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let busy = app.management.pending.is_some();
    let dialog = app.management.dialog.as_mut().expect("credential dialog");
    let state = dialog.credentials.as_ref().expect("credential state");
    let Entity::Connection(row) = &dialog.target.entity else {
        unreachable!()
    };
    let width = area.width.saturating_sub(2).min(72);
    let identity = address(row);
    let address = note_lines(&safe(&identity), width.saturating_sub(4));
    let key = if busy {
        "session-saving"
    } else if let Some(key) = dialog.editor.error.or(dialog.error) {
        key
    } else if state.status.is_none() {
        "credential-loading"
    } else {
        "credential-clear-note"
    };
    let note = note_lines(&app.i18n.text(key), width.saturating_sub(4));
    let height = 7 + address.len() as u16 + note.len() as u16;
    if area.width < 44 || height > area.height.saturating_sub(2) {
        dialog.visible = false;
        dialog.editor.invalidate_geometry();
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
        .title(app.i18n.text(dialog.kind.label(&dialog.target)))
        .style(base)
        .border_type(if app.chrome.ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        })
        .border_style(Style::default().fg(app.theme.colors().subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(safe(&dialog.target.name)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let rows = address.len() as u16;
    frame.render_widget(
        Paragraph::new(address).style(Style::default().fg(app.theme.colors().subtle)),
        Rect::new(inner.x, inner.y + 1, inner.width, rows),
    );
    let mut y = inner.y + 1 + rows;
    if let Some(status) = &state.status {
        let label = match status.state {
            CredentialState::Absent => "credential-absent",
            CredentialState::Configured { .. } => "credential-configured",
        };
        frame.render_widget(
            Paragraph::new(app.i18n.text(label))
                .style(Style::default().fg(app.theme.colors().subtle)),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
    y += 2;
    frame.render_widget(
        Paragraph::new(note).style(Style::default().fg(
            if dialog.error.is_some() || dialog.editor.error.is_some() {
                app.theme.colors().warning
            } else {
                app.theme.colors().subtle
            },
        )),
        Rect::new(
            inner.x,
            y,
            inner.width,
            inner.bottom().saturating_sub(y + 2),
        ),
    );
    let retry = state.failed;
    let focus = dialog.focus;
    let mut right = inner.right();
    for (label, command, index) in [
        ("credential-remove", Command::Save, 1),
        ("session-cancel", Command::Close, 0),
    ] {
        let text = app.i18n.text(label);
        let width = (text.width() as u16 + 2).min(inner.width / 3);
        let rect = Rect::new(right.saturating_sub(width), inner.bottom() - 1, width, 1);
        right = rect.x.saturating_sub(1);
        button(
            frame,
            app,
            rect,
            &text,
            Action::Manage(command),
            focus == index,
        );
    }
    if retry {
        let text = app.i18n.text("credential-retry");
        button(
            frame,
            app,
            Rect::new(
                inner.x,
                inner.bottom() - 1,
                right.saturating_sub(inner.x),
                1,
            ),
            &text,
            Action::Manage(Command::CredentialRetry),
            false,
        );
    }
}
