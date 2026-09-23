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

use super::{Command, Manage, provider_name};
use crate::{
    app::{Action, App},
    view::{button, safe, tone},
};
use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::{Modifier, Style},
    widgets::{Block, Paragraph},
};
use unicode_width::UnicodeWidthStr;

pub(in crate::pages::manage) fn draw(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    base: Style,
) {
    app.hits.clear();
    let picking = app.management.oauth.attempt.is_none() && app.management.oauth.existing.is_none();
    let presenting = app.management.oauth.display.is_some();
    let customizable = app.management.oauth.customizable();
    let expanded = customizable && app.management.oauth.identity.expanded;
    if !expanded {
        app.management.oauth.identity.invalidate_geometry();
    }
    let height = if expanded {
        20
    } else if picking {
        16
    } else if presenting {
        20
    } else {
        12
    };
    let dialog = app.management.dialog.as_mut().expect("OAuth dialog");
    dialog.visible = false;
    if area.width < 44 || area.height < height {
        app.management.oauth.identity.invalidate_geometry();
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let width = area.width.saturating_sub(2).min(76);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(app.i18n.text("oauth-title"))
        .title_alignment(Alignment::Center)
        .border_type(if app.chrome.ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        })
        .style(base)
        .border_style(Style::default().fg(tone::accent(app.theme.colors())));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    app.modal_area = Some(popup);
    dialog.visible = true;
    let focus = dialog.focus;
    let state = &app.management.oauth;
    let controls = state.controls();
    let status_y = inner.y
        + if expanded {
            14
        } else if customizable {
            8
        } else if picking {
            7
        } else {
            2
        };
    if !picking {
        frame.render_widget(
            Paragraph::new(provider_name(state.provider))
                .style(Style::default().add_modifier(Modifier::BOLD)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        if !state.connection_label.is_empty() {
            frame.render_widget(
                Paragraph::new(safe(&state.connection_label))
                    .style(Style::default().fg(app.theme.colors().muted)),
                Rect::new(inner.x, inner.y + 1, inner.width, 1),
            );
        }
    }
    let status = app.i18n.text(state.status());
    frame.render_widget(
        Paragraph::new(super::super::view::note_lines(&status, inner.width)).style(
            Style::default().fg(
                if state.error.is_some()
                    || (customizable && state.identity.error().is_some())
                    || matches!(
                        state.projection.as_ref().map(|p| p.phase),
                        Some(maka_protocol::oauth::Phase::Failed { .. })
                    )
                {
                    app.theme.colors().warning
                } else {
                    app.theme.colors().muted
                },
            ),
        ),
        Rect::new(inner.x, status_y, inner.width, 3),
    );
    if let Some((url, code)) = &state.display {
        frame.render_widget(
            Paragraph::new(super::super::view::note_lines(&safe(url), inner.width))
                .style(Style::default().fg(tone::accent(app.theme.colors()))),
            Rect::new(inner.x, inner.y + 6, inner.width, 3),
        );
        if let Some(code) = code {
            frame.render_widget(
                Paragraph::new(safe(code))
                    .alignment(Alignment::Center)
                    .style(
                        Style::default()
                            .fg(tone::accent(app.theme.colors()))
                            .add_modifier(Modifier::BOLD),
                    ),
                Rect::new(inner.x, inner.y + 10, inner.width, 1),
            );
        }
    }
    if state.attempt.is_some() && !state.terminal() {
        frame.render_widget(
            Paragraph::new(super::super::view::note_lines(
                &app.i18n.text("oauth-hide-note"),
                inner.width,
            ))
            .style(Style::default().fg(app.theme.colors().subtle)),
            Rect::new(
                inner.x,
                inner.bottom() - if presenting { 6 } else { 4 },
                inner.width,
                2,
            ),
        );
    }
    if customizable {
        let action = Action::Manage(Manage::Oauth(Command::Identity));
        let label = format!(
            "{} {}",
            app.chrome.symbol(
                if expanded { "▾" } else { "▸" },
                if expanded { "v" } else { ">" }
            ),
            app.i18n.text(
                if app
                    .management
                    .oauth
                    .identity
                    .fields
                    .iter()
                    .any(|field| !field.text().trim().is_empty())
                {
                    "oauth-identity-set"
                } else {
                    "oauth-identity"
                }
            )
        );
        crate::view::list_item(
            frame,
            app,
            Rect::new(inner.x, inner.y + 6, inner.width, 1),
            &label,
            action,
            controls.get(focus) == Some(&Manage::Oauth(Command::Identity)),
        );
        if expanded {
            for index in 0..2 {
                let command = Command::Field(index);
                let enabled = app.oauth_enabled(command);
                let focused = enabled && controls.get(focus) == Some(&Manage::Oauth(command));
                // Keep each label with its editor, then leave a quiet row
                // between fields instead of packing content above empty space.
                let y = inner.y + 8 + index as u16 * 3;
                let label = app.i18n.text(command.label());
                frame.render_widget(
                    Paragraph::new(label).style(Style::default().fg(if focused {
                        tone::accent(app.theme.colors())
                    } else {
                        app.theme.colors().muted
                    })),
                    Rect::new(inner.x, y, inner.width, 1),
                );
                let rect = Rect::new(inner.x + 1, y + 1, inner.width.saturating_sub(2), 1);
                frame.render_widget(
                    Paragraph::new("").style(tone::selection(app.theme.colors())),
                    rect,
                );
                let field = &mut app.management.oauth.identity.fields[index];
                field.draw(frame, rect, focused, app.theme.colors());
                if field.text().is_empty() && !focused {
                    frame.render_widget(
                        Paragraph::new(app.i18n.text("oauth-identity-default"))
                            .style(Style::default().fg(app.theme.colors().subtle)),
                        rect,
                    );
                }
                if enabled {
                    app.hits.push(crate::app::Hit {
                        area: Rect::new(inner.x, y, inner.width, 2),
                        action: Action::Manage(Manage::Oauth(command)),
                    });
                }
            }
        }
    }
    let mut right = inner.right();
    let mut copy_x = inner.x;
    for (index, command) in controls.iter().enumerate() {
        let (label, rect) = if let Manage::Oauth(Command::Provider(provider)) = command {
            let marker = if *provider == app.management.oauth.provider {
                app.chrome.symbol("●", "*")
            } else {
                " "
            };
            (
                format!("{marker} {}", provider_name(*provider)),
                Rect::new(inner.x, inner.y + *provider as u16 * 2, inner.width, 1),
            )
        } else if matches!(
            command,
            Manage::Oauth(Command::CopyLink | Command::CopyCode)
        ) {
            let label = app.i18n.text(command.label());
            let width = (label.width() as u16 + 2).min(inner.width / 2);
            let rect = Rect::new(copy_x, inner.bottom() - 4, width, 1);
            copy_x += width;
            (label, rect)
        } else {
            continue;
        };
        let draw_control = if matches!(command, Manage::Oauth(Command::Provider(_))) {
            crate::view::list_item
        } else {
            button
        };
        draw_control(
            frame,
            app,
            rect,
            &label,
            Action::Manage(command.clone()),
            focus == index,
        );
    }
    for (index, command) in controls.iter().enumerate().rev() {
        if matches!(
            command,
            Manage::Oauth(
                Command::Provider(_)
                    | Command::CopyLink
                    | Command::CopyCode
                    | Command::Identity
                    | Command::Field(_)
            )
        ) {
            continue;
        }
        let label = app.i18n.text(if *command == Manage::Close {
            "oauth-close"
        } else {
            command.label()
        });
        let width = (label.width() as u16 + 2).min(inner.width / 3);
        let rect = Rect::new(right.saturating_sub(width), inner.bottom() - 1, width, 1);
        right = rect.x.saturating_sub(1);
        button(
            frame,
            app,
            rect,
            &label,
            Action::Manage(command.clone()),
            focus == index,
        );
    }
}
