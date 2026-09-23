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
    view::{button, safe, tone},
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
    let dialog = app.management.dialog.as_mut().expect("models dialog");
    let models = dialog.models.as_mut().expect("model chooser");
    if area.width < 42 || area.height < 17 {
        dialog.visible = false;
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let width = area.width.saturating_sub(2).min(80);
    let height = area
        .height
        .saturating_sub(2)
        .min((models.catalog.rows.len().min(9) as u16 * 2 + 10).clamp(15, 28));
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
        .title(app.i18n.text(dialog.kind.label(&dialog.target)))
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    dialog.visible = true;
    if models.for_default {
        let command = Manage::Models(Command::ClearDefault);
        let marker = if models.clear_default {
            app.chrome.symbol("›", ">")
        } else {
            " "
        };
        let current = if models.catalog.revision().is_some() && !models.catalog.has_default {
            format!(" · {}", app.i18n.text("default-model-current"))
        } else {
            String::new()
        };
        let rect = Rect::new(inner.x, inner.y, inner.width, 1);
        frame.render_widget(
            Paragraph::new(format!(
                "{marker} {}{current}",
                app.i18n.text("default-model-none")
            ))
            .style(
                if models.clear_default || models.hovered.as_ref() == Some(&command) {
                    tone::selection(app.theme.colors()).fg(tone::accent(app.theme.colors()))
                } else {
                    Style::default()
                },
            ),
            rect,
        );
        if !dialog.blocked && !busy {
            app.hits.push(Hit {
                area: rect,
                action: Action::Manage(command),
            });
        }
    } else {
        frame.render_widget(
            Paragraph::new(safe(&dialog.target.name)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    let list = Rect::new(
        inner.x,
        inner.y + 3,
        inner.width,
        inner.height.saturating_sub(9),
    );
    let visible = list.height as usize / 2;
    let selected = models
        .catalog
        .rows
        .iter()
        .position(|r| Some(&r.choice) == models.catalog.selected.as_ref());
    let offset = selected.map_or(0, |i| (i + 1).saturating_sub(visible));
    let enabled = !dialog.blocked && !busy;
    for (index, row) in models
        .catalog
        .rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
    {
        let rect = Rect::new(
            list.x,
            list.y + ((index - offset) * 2) as u16,
            list.width,
            2,
        );
        let command = Manage::Models(Command::Select(row.choice.clone()));
        let active = selected == Some(index) || models.hovered.as_ref() == Some(&command);
        let marker = if selected == Some(index) {
            if app.chrome.ascii { ">" } else { "›" }
        } else {
            " "
        };
        let subtitle = if row.name == row.choice.model {
            safe(&row.connection)
        } else {
            format!("{} · {}", safe(&row.connection), safe(&row.choice.model))
        };
        frame.render_widget(
            Paragraph::new(format!(
                "{marker} {}{}",
                safe(&row.name),
                if models.for_default && row.is_default {
                    format!(" · {}", app.i18n.text("default-model-current"))
                } else {
                    String::new()
                }
            ))
            .style(if !enabled {
                Style::default().fg(app.theme.colors().subtle)
            } else if active {
                tone::selection(app.theme.colors()).fg(tone::accent(app.theme.colors()))
            } else {
                Style::default()
            }),
            Rect::new(rect.x, rect.y, rect.width, 1),
        );
        frame.render_widget(
            Paragraph::new(format!("  {subtitle}"))
                .style(Style::default().fg(app.theme.colors().subtle)),
            Rect::new(rect.x, rect.y + 1, rect.width, 1),
        );
        if enabled {
            app.hits.push(Hit {
                area: rect,
                action: Action::Manage(command),
            });
        }
    }
    if models.catalog.rows.is_empty() && !models.catalog.error {
        frame.render_widget(
            Paragraph::new(app.i18n.text(if models.catalog.ready() {
                if models.catalog.can_previous() {
                    "session-model-page-empty"
                } else {
                    "session-model-empty"
                }
            } else {
                "session-model-loading"
            }))
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(app.theme.colors().subtle)),
            list,
        );
    }
    let key = if busy {
        "session-saving"
    } else if let Some(error) = dialog.error {
        error
    } else if models.catalog.error {
        "session-model-load-failed"
    } else {
        let command = models.hovered.clone().unwrap_or_else(|| focused(models));
        match command {
            Manage::Models(Command::Refresh | Command::Previous | Command::Next) => command.label(),
            _ if models.for_default && models.clear_default => "default-model-clear-note",
            _ if models.for_default => "default-model-note",
            _ if models.selection().is_some()
                && models.thinking.is_some()
                && models.thinking_level().is_none() =>
            {
                "session-thinking-fallback"
            }
            _ => "session-model-note",
        }
    };
    frame.render_widget(
        Paragraph::new(super::super::view::note_lines(
            &app.i18n.text(key),
            inner.width,
        ))
        .style(
            Style::default().fg(if dialog.error.is_some() || models.catalog.error {
                app.theme.colors().warning
            } else {
                app.theme.colors().subtle
            }),
        ),
        Rect::new(inner.x, inner.bottom() - 5, inner.width, 3),
    );
    let focus = models.focus;
    let hovered = models.hovered.clone();
    let thinking = models.has_thinking().then(|| {
        format!(
            "{}  {} {} {}",
            app.i18n.text("session-thinking"),
            app.chrome.symbol("‹", "<"),
            app.i18n.text(super::thinking_key(models.thinking_level())),
            app.chrome.symbol("›", ">")
        )
    });
    let save = app.i18n.text(if models.for_default {
        if models.clear_default {
            "default-model-clear"
        } else {
            "default-model-apply"
        }
    } else {
        "session-model-apply"
    });
    let cancel = app.i18n.text("session-cancel");
    let save_width = (save.width() as u16 + 2).min(inner.width / 2);
    let cancel_width = (cancel.width() as u16 + 2).min(inner.width / 2);
    let save_rect = Rect::new(
        inner.right() - save_width,
        inner.bottom() - 1,
        save_width,
        1,
    );
    if let Some(text) = thinking {
        let command = Manage::Models(Command::Thinking(true));
        let previous = Manage::Models(Command::Thinking(false));
        let rect = Rect::new(
            inner.x,
            inner.bottom() - 6,
            (text.width() as u16).min(inner.width),
            1,
        );
        crate::view::list_item(
            frame,
            app,
            rect,
            &text,
            Action::Manage(command.clone()),
            focus == 6 || hovered.as_ref() == Some(&command) || hovered.as_ref() == Some(&previous),
        );
        if app.enabled(&Action::Manage(previous.clone())) {
            // The left chevron reverses; the value/right chevron advances.
            app.hits.push(Hit {
                area: Rect::new(
                    rect.x + app.i18n.text("session-thinking").width() as u16 + 1,
                    rect.y,
                    3,
                    1,
                ),
                action: Action::Manage(previous),
            });
        }
    }
    for (rect, text, command, index) in [
        (
            Rect::new(save_rect.x - cancel_width - 1, save_rect.y, cancel_width, 1),
            cancel,
            Manage::Close,
            4,
        ),
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
        let command = Manage::Models(command);
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
