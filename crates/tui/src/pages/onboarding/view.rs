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

use super::{Command, PROVIDERS};
use crate::{
    app::{Action, App, Hit},
    view::{button, safe},
};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    widgets::{Block, Borders, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    app.hits.clear();
    let busy = app.onboarding.pending.is_some();
    let f = app.onboarding.dialog.as_mut().expect("onboarding form");
    if area.width < 44 || area.height < 22 {
        f.visible = false;
        for field in &mut f.fields {
            field.invalidate_geometry();
        }
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    f.visible = true;
    let width = area.width.saturating_sub(2).min(76);
    let height = area.height.saturating_sub(2).min(
        f.models
            .as_ref()
            .map_or(20, |models| (models.len().min(16) as u16 + 10).max(12)),
    );
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(app.i18n.text("onboard-title"))
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
    let focus = f.focus;
    let models = f.models.is_some();
    let error = f.error.or_else(|| f.fields.iter().find_map(|e| e.error));
    if let Some(list) = &f.models {
        frame.render_widget(
            Paragraph::new(app.i18n.text("onboard-models")),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        let visible = inner.height.saturating_sub(7) as usize;
        let start = (f.row + 1).saturating_sub(visible);
        for (index, model) in list.iter().enumerate().skip(start).take(visible) {
            let selected = f.selected.contains(&model.id);
            let name = model.display_name.as_deref().unwrap_or(&model.id);
            let marker = if selected { "[x]" } else { "[ ]" };
            let label = if name == model.id {
                safe(name)
            } else {
                format!("{} · {}", safe(name), safe(&model.id))
            };
            let text = format!("{marker} {label}");
            let rect = Rect::new(
                inner.x,
                inner.y + 2 + (index - start) as u16,
                inner.width,
                1,
            );
            frame.render_widget(
                Paragraph::new(text).style(
                    if !busy && !f.blocked && focus == 0 && index == f.row {
                        app.theme.colors().selected()
                    } else {
                        Style::default()
                    },
                ),
                rect,
            );
            if !busy && !f.blocked {
                app.hits.push(Hit {
                    area: rect,
                    action: Action::Onboard(Command::Toggle(model.id.clone())),
                });
            }
        }
    } else {
        frame.render_widget(
            Paragraph::new(app.i18n.text("onboard-provider"))
                .style(Style::default().fg(app.theme.colors().subtle)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        for (index, key) in ["onboard-name", "onboard-url", "onboard-key"]
            .into_iter()
            .enumerate()
        {
            let key = if index == 1 && f.provider == 0 {
                "onboard-url-custom"
            } else {
                key
            };
            let y = inner.y + 3 + index as u16 * 3;
            frame.render_widget(
                Paragraph::new(app.i18n.text(key))
                    .style(Style::default().fg(app.theme.colors().subtle)),
                Rect::new(inner.x, y, inner.width, 1),
            );
            let rect = Rect::new(inner.x, y + 1, inner.width, 1);
            frame.render_widget(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(
                        if focus == index + 1 && !busy && !f.blocked {
                            app.theme.colors().accent
                        } else {
                            app.theme.colors().subtle
                        },
                    )),
                Rect::new(rect.x, rect.y, rect.width, 2),
            );
            let field = &mut f.fields[index];
            if index == 2 {
                field.draw_masked(
                    frame,
                    rect,
                    focus == index + 1 && !busy && !f.blocked,
                    app.theme.colors(),
                );
            } else {
                field.draw(
                    frame,
                    rect,
                    focus == index + 1 && !busy && !f.blocked,
                    app.theme.colors(),
                );
            }
            if !busy && !f.blocked {
                app.hits.push(Hit {
                    area: Rect::new(rect.x, rect.y - 1, rect.width, 2),
                    action: Action::Onboard(Command::Field(index)),
                });
            }
        }
    }
    let key = if busy {
        "onboard-busy"
    } else {
        error.unwrap_or(if models {
            "onboard-models-note"
        } else {
            "onboard-verify-note"
        })
    };
    frame.render_widget(
        Paragraph::new(crate::pages::manage::view::note_lines(
            &app.i18n.text(key),
            inner.width,
        ))
        .style(Style::default().fg(if error.is_some() {
            app.theme.colors().warning
        } else {
            app.theme.colors().subtle
        })),
        Rect::new(inner.x, inner.bottom() - 5, inner.width, 3),
    );
    let provider = PROVIDERS[f.provider].1;
    if !models {
        crate::view::list_item(
            frame,
            app,
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
            &format!("{provider} {}", app.chrome.symbol("›", ">")),
            Action::Onboard(Command::Provider),
            focus == 0,
        );
    }
    let controls = if models {
        vec![(Command::Save, 3), (Command::Close, 2), (Command::Back, 1)]
    } else {
        vec![(Command::Verify, 4), (Command::Close, 5)]
    };
    let mut right = inner.right();
    for (command, index) in controls {
        let label = app.i18n.text(command.label());
        let width = (label.width() as u16 + 2).min(inner.width / 3);
        let rect = Rect::new(right.saturating_sub(width), inner.bottom() - 1, width, 1);
        right = rect.x.saturating_sub(1);
        button(
            frame,
            app,
            rect,
            &label,
            Action::Onboard(command),
            focus == index,
        );
    }
}
