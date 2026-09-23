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
    app::{Action, App, ConnectionState, Focus, Hit},
    view::safe,
};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let area = area.inner(Margin::new(1, u16::from(area.height >= 18)));
    let state = &app.connections;
    let message = if !matches!(app.connection, ConnectionState::Connected { .. }) {
        Some("workspace-connect")
    } else if state.error {
        Some("connections-failed")
    } else if !state.loaded {
        Some("connections-loading")
    } else if state.rows.is_empty() {
        Some("connections-empty")
    } else {
        None
    };
    if let Some(key) = message {
        frame.render_widget(
            Paragraph::new(app.i18n.text(key)).wrap(Wrap { trim: false }),
            area,
        );
        return; // No stale directory behind a disconnect or read error.
    }
    let visible = usize::from(area.height / 3);
    let selected = state
        .rows
        .iter()
        .position(|row| Some(&row.id) == state.selected.as_ref());
    let offset = selected.map_or(0, |index| (index + 1).saturating_sub(visible));
    let muted = Style::default().fg(if app.theme.choice == crate::theme::Choice::Terminal {
        app.theme.colors().muted
    } else {
        app.theme.colors().subtle
    });
    for (index, row) in state.rows.iter().enumerate().skip(offset).take(visible) {
        let action = Action::Connection(Command::Select(row.id.clone()));
        let focused = app.palette.is_none()
            && (app.focus == Focus::List && selected == Some(index)
                || app.hover.as_ref() == Some(&action));
        let rect = Rect::new(
            area.x,
            area.y + ((index - offset) * 3) as u16,
            area.width,
            2,
        );
        let name = format!(
            "{} {}",
            if selected == Some(index) {
                app.chrome.symbol("›", ">")
            } else {
                " "
            },
            safe(&row.name)
        );
        let status = if row.enabled {
            "".into()
        } else {
            format!(" · {}", app.i18n.text("connection-disabled"))
        };
        let title = Line::from(vec![
            Span::styled(
                name,
                if focused {
                    Style::default().fg(app.theme.colors().accent)
                } else {
                    Style::default()
                },
            ),
            Span::styled(status, muted),
        ]);
        let identity = if row.slug == row.provider {
            safe(&row.slug)
        } else {
            format!("{} · {}", safe(&row.slug), safe(&row.provider))
        };
        let detail = if let Some(model) = &row.default_model {
            format!(
                "  {}: {} · {}",
                app.i18n.text("connection-default"),
                safe(model),
                identity
            )
        } else {
            format!(
                "  {} · {}",
                app.i18n.format(
                    "connection-models",
                    &[("count", &row.enabled_models.to_string())]
                ),
                identity
            )
        };
        frame.render_widget(
            Paragraph::new(vec![title, Line::styled(detail, muted)]),
            rect,
        );
        app.hits.push(Hit { area: rect, action });
    }
}
