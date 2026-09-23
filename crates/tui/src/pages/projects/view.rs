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
    layout::Rect,
    style::Style,
    widgets::{Paragraph, Wrap},
};

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let area = area.inner(ratatui::layout::Margin::new(
        1,
        u16::from(area.height >= 18),
    ));
    let state = &app.projects;
    let message = if !matches!(app.connection, ConnectionState::Connected { .. }) {
        Some("workspace-connect")
    } else if state.error {
        Some("projects-failed")
    } else if state.items.is_empty() {
        Some(if state.loading {
            "projects-loading"
        } else if state.can_next() || state.can_previous() {
            "projects-page-empty"
        } else {
            "projects-empty"
        })
    } else {
        None
    };
    let mut list = area;
    if let Some(key) = message {
        frame.render_widget(
            Paragraph::new(app.i18n.text(key)).wrap(Wrap { trim: false }),
            Rect::new(area.x, area.y, area.width, 2.min(area.height)),
        );
        list.y += 2.min(list.height);
        list.height = list.height.saturating_sub(2);
    }
    let visible = list.height as usize / 2;
    let selected = state
        .items
        .iter()
        .position(|item| Some(&item.id) == state.selected.as_ref());
    let offset = selected.map_or(0, |index| (index + 1).saturating_sub(visible));
    for (index, item) in state.items.iter().enumerate().skip(offset).take(visible) {
        let action = Action::Project(Command::Select(item.id.clone()));
        let active = app.palette.is_none()
            && (app.focus == Focus::List && selected == Some(index)
                || app.hover.as_ref() == Some(&action));
        let rect = Rect::new(
            list.x,
            list.y + ((index - offset) * 2) as u16,
            list.width,
            1,
        );
        let status = if item.archived {
            Some("session-archived")
        } else if !item.available {
            Some("project-unavailable")
        } else {
            None
        };
        let text = format!(
            "{}{}",
            safe(&item.name),
            status.map_or(String::new(), |key| format!(" · {}", app.i18n.text(key)))
        );
        frame.render_widget(
            Paragraph::new(text).style(if active {
                app.theme.colors().selected()
            } else {
                Style::default()
            }),
            rect,
        );
        app.hits.push(Hit { area: rect, action });
    }
}
