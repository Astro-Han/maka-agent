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

use super::{Command, Manage};
use crate::{
    app::{Action, App, Hit},
    view::{button, safe, tone},
};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    widgets::{Block, BorderType, Paragraph},
};
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    if app
        .management
        .dialog
        .as_ref()
        .and_then(|dialog| dialog.enabled_models.as_ref())
        .is_some_and(|state| state.profile.is_some())
    {
        super::profile::draw(frame, app, area, base);
        return;
    }
    let busy = app.management.pending.is_some();
    let dialog = app.management.dialog.as_mut().unwrap();
    let state = dialog.enabled_models.as_mut().unwrap();
    if area.width < 42 || area.height < 17 {
        dialog.visible = false;
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let width = area.width.saturating_sub(2).min(80);
    let height = area
        .height
        .saturating_sub(2)
        .min((state.catalog.rows.len() as u16 + 12).clamp(15, 28));
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .title(app.i18n.text(if state.edit_profiles {
            "connection-model-overrides"
        } else {
            "connection-enabled-models"
        }))
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    dialog.visible = true;
    let enabled = !dialog.blocked && !busy;
    let search = Rect::new(inner.x + 2, inner.y, inner.width.saturating_sub(2), 1);
    frame.render_widget(
        Paragraph::new(app.chrome.symbol("⌕", "/"))
            .style(Style::default().fg(tone::secondary(app.theme.colors()))),
        Rect::new(inner.x, inner.y, 2, 1),
    );
    state.search.draw(
        frame,
        search,
        enabled && state.focus == 0,
        app.theme.colors(),
    );
    if enabled {
        app.hits.push(Hit {
            area: Rect::new(inner.x, inner.y, 2, 1),
            action: Action::Manage(Manage::EnabledModels(Command::Search)),
        });
    }
    if state.search.text().is_empty() {
        frame.render_widget(
            Paragraph::new(app.i18n.text("enabled-model-search"))
                .style(Style::default().fg(app.theme.colors().subtle)),
            search,
        );
    }
    let list = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(8),
    );
    let filtered = state.filtered();
    let visible = list.height as usize;
    let offset = (state.row + 1).saturating_sub(visible);
    for (position, index) in filtered.iter().enumerate().skip(offset).take(visible) {
        let model = &state.catalog.rows[*index];
        let command = Manage::EnabledModels(Command::Toggle(model.id.clone()));
        let chosen = state.selected.contains(&model.id);
        let text = if state.edit_profiles {
            format!(
                "{} {}",
                safe(&model.id),
                if model.name == model.id {
                    String::new()
                } else {
                    format!("· {}", safe(&model.name))
                }
            )
        } else {
            format!(
                "[{}] {}{}",
                if chosen {
                    app.chrome.symbol("✓", "x")
                } else {
                    " "
                },
                safe(&model.id),
                if model.name == model.id {
                    String::new()
                } else {
                    format!(" · {}", safe(&model.name))
                }
            )
        };
        let rect = Rect::new(list.x, list.y + (position - offset) as u16, list.width, 1);
        let style = if !enabled || !state.catalog.ready {
            Style::default().fg(app.theme.colors().subtle)
        } else if (state.focus == 1 && position == state.row)
            || state.hovered.as_ref() == Some(&command)
        {
            tone::selection(app.theme.colors()).fg(tone::accent(app.theme.colors()))
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(text).style(style), rect);
        if enabled && state.catalog.ready {
            app.hits.push(Hit {
                area: rect,
                action: Action::Manage(command),
            });
        }
    }
    if filtered.is_empty() {
        frame.render_widget(
            Paragraph::new(app.i18n.text(if state.catalog.ready {
                "enabled-model-empty"
            } else {
                "session-model-loading"
            }))
            .style(Style::default().fg(app.theme.colors().subtle)),
            list,
        );
    }
    let focus = state.focus;
    let edit_profiles = state.edit_profiles;
    let hovered = state.hovered.clone();
    let clears_default = state
        .catalog
        .basis
        .default_model
        .as_ref()
        .is_some_and(|id| !state.selected.contains(id));
    let error = dialog.error.or(state.catalog.error).or(state.search.error);
    let note = if busy {
        app.i18n.text("session-saving")
    } else if let Some(error) = error {
        app.i18n.text(error)
    } else if !state.catalog.ready {
        app.i18n.text("session-model-loading")
    } else if state.edit_profiles {
        app.i18n.text("model-profile-choose")
    } else if clears_default {
        app.i18n.text("enabled-model-default-note")
    } else {
        app.i18n.format(
            "enabled-model-note",
            &[("count", &state.selected.len().to_string())],
        )
    };
    let lines = super::super::view::note_lines(&note, inner.width);
    dialog.visible = lines.len() <= 4;
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(if error.is_some() || clears_default {
            app.theme.colors().warning
        } else {
            tone::secondary(app.theme.colors())
        })),
        Rect::new(inner.x, inner.bottom() - 5, inner.width, 4),
    );
    let save = app.i18n.text("session-save");
    let cancel = app.i18n.text("session-cancel");
    let save_width = if edit_profiles {
        0
    } else {
        save.width() as u16 + 2
    };
    let cancel_width = cancel.width() as u16 + 2;
    let y = inner.bottom() - 1;
    button(
        frame,
        app,
        Rect::new(
            inner.right() - save_width - cancel_width - 1,
            y,
            cancel_width,
            1,
        ),
        &cancel,
        Action::Manage(Manage::Close),
        focus == 2 || hovered == Some(Manage::Close),
    );
    if !edit_profiles {
        button(
            frame,
            app,
            Rect::new(inner.right() - save_width, y, save_width, 1),
            &save,
            Action::Manage(Manage::Save),
            focus == 3 || hovered == Some(Manage::Save),
        );
    }
    if error == Some("session-model-load-failed") {
        button(
            frame,
            app,
            Rect::new(inner.x, y, 3, 1),
            app.chrome.symbol("⟳", "R"),
            Action::Manage(Manage::EnabledModels(Command::Retry)),
            false,
        );
    }
}
