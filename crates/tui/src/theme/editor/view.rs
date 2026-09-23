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

use super::{Command, SWATCHES};
use crate::app::{Action, App, Hit};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
};

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    app.hits.clear();
    let busy = app.theme.busy();
    let error = app.theme.error_text(&app.i18n);
    let path = app
        .theme
        .path
        .as_ref()
        .map(|path| crate::view::safe(&path.to_string_lossy()));
    let editor = app.theme.editor.as_mut().expect("theme editor");
    // Preview may temporarily make foreground and background identical. Keep
    // controls readable so Cancel/Save never disappear while tuning.
    let colors = editor.chrome;
    if area.width < 44 || area.height < 24 {
        editor.invalidate();
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    editor.visible = true;
    let width = area.width.saturating_sub(2).min(78);
    let height = area.height.saturating_sub(2).min(28);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    let block = Block::bordered()
        .title(app.i18n.text("theme-customize"))
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .style(colors.base())
        .border_style(Style::default().fg(colors.accent));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    frame.render_widget(block, popup);
    let name = Rect::new(inner.x + 8, inner.y, inner.width - 8, 1);
    frame.render_widget(
        Paragraph::new(app.i18n.text("theme-name")).style(Style::default().fg(colors.muted)),
        Rect::new(inner.x, inner.y, 8, 1),
    );
    editor
        .name
        .draw(frame, name, !busy && editor.focus == 0, colors);
    let mut controls = Vec::new();
    for (index, label) in ["Maka", "Dusk", "Paper"].into_iter().enumerate() {
        controls.push((
            Rect::new(inner.x + index as u16 * 9, inner.y + 2, 8, 1),
            label.to_owned(),
            Command::Base(index),
            editor.focus == 1 && editor.base == index,
        ));
    }
    let left_width = 18u16.min(inner.width / 2);
    let visible = usize::from(inner.height.saturating_sub(9));
    let start = (editor.role + 1).saturating_sub(visible);
    let entries = editor.colors.entries();
    for (index, (key, color)) in entries.iter().enumerate().skip(start).take(visible) {
        let y = inner.y + 4 + (index - start) as u16;
        frame.render_widget(
            Paragraph::new("  ").style(Style::default().bg(*color)),
            Rect::new(inner.x, y, 2, 1),
        );
        let rect = Rect::new(inner.x + 3, y, left_width - 4, 1);
        let style = if index == editor.role {
            colors.selected()
        } else {
            Style::default().fg(colors.muted)
        };
        frame.render_widget(
            Paragraph::new(app.i18n.text(&format!("theme-role-{key}"))).style(style),
            rect,
        );
        if !busy {
            app.hits.push(Hit {
                area: Rect::new(inner.x, y, left_width, 1),
                action: Action::Theme(Command::Role(index)),
            });
        }
    }
    if visible < entries.len() {
        let x = inner.x + left_width - 1;
        for row in 0..visible {
            let selected = row == editor.role * visible / entries.len();
            frame.render_widget(
                Paragraph::new(if selected {
                    app.chrome.symbol("┃", "#")
                } else {
                    app.chrome.symbol("│", "|")
                })
                .style(Style::default().fg(if selected {
                    colors.scrollbar
                } else {
                    colors.border
                })),
                Rect::new(x, inner.y + 4 + row as u16, 1, 1),
            );
        }
    }
    let right = Rect::new(
        inner.x + left_width + 2,
        inner.y + 4,
        inner.width - left_width - 2,
        12,
    );
    frame.render_widget(
        Paragraph::new(app.i18n.text("theme-swatches")).style(Style::default().fg(colors.muted)),
        Rect::new(right.x, right.y, right.width, 1),
    );
    let cell = right.width / 6;
    for (index, hex) in SWATCHES.iter().enumerate() {
        let rect = Rect::new(
            right.x + index as u16 % 6 * cell,
            right.y + 2 + index as u16 / 6 * 2,
            cell - 1,
            1,
        );
        let color = super::super::rgb(*hex);
        let selected = color == entries[editor.role].1;
        let Color::Rgb(r, g, b) = color else {
            unreachable!()
        };
        let ink = if u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 > 145000 {
            Color::Black
        } else {
            Color::White
        };
        let mut style = Style::default().bg(color).fg(ink);
        if editor.focus == 3 && editor.swatch == index {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        frame.render_widget(
            Paragraph::new(if selected {
                if app.chrome.ascii { "*" } else { "✓" }
            } else {
                " "
            })
            .centered()
            .style(style),
            rect,
        );
        if !busy {
            app.hits.push(Hit {
                area: rect,
                action: Action::Theme(Command::Swatch(index)),
            });
        }
    }
    frame.render_widget(
        Paragraph::new("#RRGGBB").style(Style::default().fg(colors.muted)),
        Rect::new(right.x, right.y + 8, right.width, 1),
    );
    editor.hex.draw(
        frame,
        Rect::new(right.x, right.y + 9, right.width, 1),
        !busy && editor.focus == 4,
        colors,
    );
    if editor.error.is_none() && error.is_none() {
        frame.render_widget(
            Paragraph::new(app.i18n.text("theme-preview")).style(
                Style::default()
                    .fg(editor.colors.foreground)
                    .bg(editor.colors.surface),
            ),
            Rect::new(right.x, right.y + 11, right.width, 1),
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("fn", Style::default().fg(editor.colors.syntax[0])),
                Span::styled(" main", Style::default().fg(editor.colors.syntax[4])),
                Span::styled("() { ", Style::default().fg(editor.colors.syntax[6])),
                Span::styled("42", Style::default().fg(editor.colors.syntax[3])),
                Span::styled(" }", Style::default().fg(editor.colors.syntax[6])),
            ]))
            .style(Style::default().bg(editor.colors.surface)),
            Rect::new(right.x, right.y + 12, right.width, 1),
        );
    }
    if let Some(error) = editor.error.map(|key| app.i18n.text(key)).or(error) {
        frame.render_widget(
            Paragraph::new(error)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(colors.warning)),
            Rect::new(inner.x, inner.bottom() - 5, inner.width, 2),
        );
    }
    if let Some(path) = path {
        frame.render_widget(
            Paragraph::new(path).style(Style::default().fg(colors.subtle)),
            Rect::new(inner.x, inner.bottom() - 3, inner.width, 1),
        );
    }
    let focus = editor.focus;
    let button_width = (inner.width / 3).min(18);
    for (index, command) in [Command::Reload, Command::Close, Command::Save]
        .into_iter()
        .enumerate()
    {
        controls.push((
            Rect::new(
                inner.x + index as u16 * (inner.width / 3),
                inner.bottom() - 1,
                button_width,
                1,
            ),
            app.i18n.text(command.label()),
            command,
            focus == index + 5,
        ));
    }
    for (rect, label, command, focused) in controls {
        let enabled = !busy || command == Command::Close;
        let style = if !enabled {
            Style::default()
                .fg(colors.subtle)
                .add_modifier(Modifier::DIM)
        } else if focused {
            colors.selected()
        } else {
            Style::default().fg(colors.muted)
        };
        frame.render_widget(Paragraph::new(label).centered().style(style), rect);
        if enabled {
            app.hits.push(Hit {
                area: rect,
                action: Action::Theme(command),
            });
        }
    }
}
