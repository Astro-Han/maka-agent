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

use super::*;
use crate::app::Hit;
use crate::view::{button, safe};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn controls(app: &App) -> Vec<Command> {
    let dialog = app.attachments.dialog.as_ref().unwrap();
    if dialog.browse {
        let mut commands = vec![Command::Close];
        if app.attachments.has(&dialog.session) {
            commands.push(Command::Open);
        }
        commands
    } else {
        let mut commands = vec![Command::Close, Command::Browse];
        if app.attachment_enabled(&Command::Retry) {
            commands.push(Command::Retry);
        }
        if app.attachments.has(&dialog.session) {
            commands.push(Command::Remove);
        }
        commands
    }
}
pub fn size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}
impl App {
    pub fn attachment_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let buttons = controls(self);
        let state = &mut self.attachments;
        let dialog = state.dialog.as_mut().unwrap();
        let count = if dialog.browse {
            dialog.entries.len()
        } else {
            state.saved.get(&dialog.session).map_or(0, Vec::len)
        };
        let last = count.saturating_sub(1);
        let capacity = dialog.list.map_or(1, |a| {
            usize::from(a.height / if dialog.browse { 2 } else { 3 }).max(1)
        });
        let mut command = None;
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return (true, Some(Action::Quit));
                }
                KeyCode::Esc => command = Some(Command::Close),
                _ if !dialog.rendered => return (false, None),
                KeyCode::Tab | KeyCode::BackTab => {
                    let first = usize::from(!dialog.browse);
                    let slots = buttons.len() + 2 - first;
                    dialog.focus = if key.code == KeyCode::Tab {
                        first + (dialog.focus.saturating_sub(first) + 1) % slots
                    } else {
                        first + (dialog.focus.saturating_sub(first) + slots - 1) % slots
                    };
                }
                KeyCode::Enter if dialog.focus == 0 => command = Some(Command::EnterPath),
                KeyCode::Enter if dialog.focus >= 2 => {
                    command = buttons.get(dialog.focus - 2).cloned()
                }
                _ if dialog.focus == 0 => {
                    dialog.path.key(key);
                }
                KeyCode::Up => {
                    dialog.selected = dialog.selected.saturating_sub(1);
                    dialog.focus = 1;
                }
                KeyCode::Down => {
                    dialog.selected = (dialog.selected + 1).min(last);
                    dialog.focus = 1;
                }
                KeyCode::PageUp => dialog.selected = dialog.selected.saturating_sub(capacity),
                KeyCode::PageDown => dialog.selected = (dialog.selected + capacity).min(last),
                KeyCode::Home => dialog.selected = 0,
                KeyCode::End => dialog.selected = last,
                KeyCode::Enter if dialog.browse => command = Some(Command::Pick(dialog.selected)),
                KeyCode::Backspace if dialog.browse => command = Some(Command::Parent),
                KeyCode::Delete if !dialog.browse => command = Some(Command::Remove),
                KeyCode::Char('r') if !dialog.browse => command = Some(Command::Retry),
                KeyCode::Char('d') | KeyCode::Enter if !dialog.browse => {
                    command = Some(Command::Details)
                }
                _ => {}
            },
            Event::Paste(text) if dialog.rendered && dialog.focus == 0 => {
                dialog.path.insert(&text.replace(['\n', '\r'], ""));
            }
            Event::Mouse(mouse) if dialog.rendered => {
                let point = (mouse.column, mouse.row).into();
                if dialog.browse && (dialog.path.contains(point) || dialog.path.dragging()) {
                    dialog.focus = 0;
                    dialog.path.mouse(mouse);
                } else if let Some(area) = dialog.list {
                    let in_list = area.contains(point);
                    match mouse.kind {
                        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if in_list => {
                            dialog.focus = 1;
                            dialog.selected = if mouse.kind == MouseEventKind::ScrollDown {
                                (dialog.selected + 2).min(last)
                            } else {
                                dialog.selected.saturating_sub(2)
                            };
                        }
                        MouseEventKind::Down(MouseButton::Left)
                            if in_list && mouse.column == area.right() - 1 && count > capacity =>
                        {
                            dialog.dragging = true;
                        }
                        MouseEventKind::Drag(MouseButton::Left) if dialog.dragging => {}
                        MouseEventKind::Up(MouseButton::Left) if dialog.dragging => {
                            dialog.dragging = false;
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            command = self
                                .hits
                                .iter()
                                .rev()
                                .find(|hit| hit.area.contains(point))
                                .and_then(|hit| {
                                    if let Action::Attachment(command) = &hit.action {
                                        Some(command.clone())
                                    } else {
                                        None
                                    }
                                });
                        }
                        _ => {}
                    }
                    if dialog.dragging {
                        dialog.top = usize::from(
                            mouse
                                .row
                                .saturating_sub(area.y)
                                .min(area.height.saturating_sub(1)),
                        ) * count.saturating_sub(capacity)
                            / usize::from(area.height.saturating_sub(1).max(1));
                        dialog.selected = dialog.top;
                        dialog.focus = 1;
                    }
                }
            }
            _ => return (false, None),
        }
        (
            true,
            command.and_then(|command| self.apply(Action::Attachment(command))),
        )
    }
}

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    let width = area.width.saturating_sub(2).min(82);
    let dialog = app.attachments.dialog.as_ref().unwrap();
    let desired = if dialog.browse {
        dialog.entries.len().clamp(1, 8) as u16 * 2 + 10
    } else {
        app.attachments
            .saved
            .get(&dialog.session)
            .map_or(1, |items| items.len().max(1)) as u16
            * 3
            + 8
    };
    let height = area.height.saturating_sub(2).min(desired);
    if width < 28 || height < 9 {
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    app.hits.clear();
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    app.modal_area = Some(popup);
    let browse = app.attachments.dialog.as_ref().unwrap().browse;
    let block = Block::bordered()
        .title(app.i18n.text(if browse {
            "attachments-local-files"
        } else {
            "attachments-title"
        }))
        .title_alignment(Alignment::Center)
        .border_type(if app.chrome.ascii {
            BorderType::Plain
        } else {
            BorderType::Rounded
        })
        .border_style(Style::default().fg(app.theme.colors().accent))
        .style(base);
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    let colors = app.theme.colors();
    let state = &mut app.attachments;
    let dialog = state.dialog.as_mut().unwrap();
    dialog.rendered = true;
    let mut top = inner.y;
    if browse {
        let path = Rect::new(inner.x + 3, top, inner.width.saturating_sub(6), 1);
        dialog.path.draw(frame, path, dialog.focus == 0, colors);
        app.hits.push(Hit {
            area: path,
            action: Action::Attachment(Command::Path),
        });
        top += 2;
    }
    let footer = inner.bottom().saturating_sub(3);
    let list = Rect::new(inner.x, top, inner.width, footer.saturating_sub(top + 2));
    dialog.list = Some(list);
    let stride = if browse { 2 } else { 3 };
    let capacity = usize::from(list.height / stride).max(1);
    let items = state
        .saved
        .get(&dialog.session)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let count = if browse {
        dialog.entries.len()
    } else {
        items.len()
    };
    dialog.selected = dialog.selected.min(count.saturating_sub(1));
    if dialog.selected < dialog.top {
        dialog.top = dialog.selected;
    }
    if dialog.selected >= dialog.top + capacity {
        dialog.top = dialog.selected + 1 - capacity;
    }
    dialog.top = dialog.top.min(count.saturating_sub(capacity));
    let selected = dialog.selected;
    let start = dialog.top;
    let focus = dialog.focus;
    let problem = dialog.problem.as_ref().map(Failure::key);
    let truncated = dialog.truncated;
    let show_details = dialog.details;
    for (position, index) in (start..count).take(capacity).enumerate() {
        let row = Rect::new(
            list.x,
            list.y + position as u16 * stride,
            list.width.saturating_sub(1),
            stride.min(list.height.saturating_sub(position as u16 * stride)),
        );
        if row.height == 0 {
            continue;
        }
        let (title, subtitle, warning) = if browse {
            let entry = &dialog.entries[index];
            (
                format!(
                    "{} {}",
                    if entry.directory {
                        if app.chrome.ascii { ">" } else { "▸" }
                    } else {
                        "·"
                    },
                    safe(&entry.path.file_name().unwrap_or_default().to_string_lossy())
                ),
                if entry.directory {
                    String::new()
                } else {
                    size(entry.bytes)
                },
                false,
            )
        } else {
            let item = &items[index];
            let (key, progress) = if item.attachment.is_some() {
                ("attachments-ready", None)
            } else if let Some(active) = state.active.as_ref().filter(|a| a.ticket.id == item.id) {
                match active.phase {
                    Phase::Reading => ("attachments-reading", None),
                    Phase::Checkpoint => ("attachments-saving", None),
                    Phase::Uploading => (
                        "attachments-uploading",
                        Some(active.transfer.bytes.load(Ordering::Relaxed)),
                    ),
                }
            } else if let Some(error) = state.errors.get(&item.id) {
                (error.key(), None)
            } else if state.queued.iter().any(|(_, id)| id == &item.id) {
                ("attachments-queued", None)
            } else {
                ("attachments-paused", None)
            };
            let name = item
                .manifest
                .as_ref()
                .map(|m| m.name.as_str())
                .unwrap_or_else(|| item.path.file_name().and_then(|s| s.to_str()).unwrap_or(""));
            let amount = item
                .manifest
                .as_ref()
                .map(|m| match progress {
                    Some(bytes) => format!("{} / {}", size(bytes), size(m.bytes)),
                    None => size(m.bytes),
                })
                .unwrap_or_default();
            (
                safe(name),
                format!(
                    "{}{}{}",
                    app.i18n.text(key),
                    if amount.is_empty() { "" } else { " · " },
                    amount
                ),
                state.errors.contains_key(&item.id),
            )
        };
        let selected_row = index == selected;
        let style = Style::default()
            .fg(if selected_row {
                colors.accent
            } else {
                colors.foreground
            })
            .bg(if selected_row {
                colors.selection
            } else {
                colors.background
            });
        frame.render_widget(
            Paragraph::new(title).style(style),
            Rect::new(row.x, row.y, row.width, 1),
        );
        if row.height > 1 {
            frame.render_widget(
                Paragraph::new(subtitle).style(Style::default().fg(if warning {
                    colors.error
                } else {
                    colors.subtle
                })),
                Rect::new(row.x + 2, row.y + 1, row.width.saturating_sub(2), 1),
            );
        }
        app.hits.push(Hit {
            area: row,
            action: Action::Attachment(if browse {
                Command::Pick(index)
            } else {
                Command::Select(index)
            }),
        });
    }
    if count > capacity {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::default().fg(colors.subtle)),
            list,
            &mut ScrollbarState::new(count.saturating_sub(capacity) + 1).position(start),
        );
    }
    let note = if let Some(problem) = problem {
        app.i18n.text(problem)
    } else if browse && truncated {
        app.i18n.text("attachments-truncated")
    } else if !browse {
        items
            .get(selected)
            .map(|item| {
                if show_details {
                    state
                        .errors
                        .get(&item.id)
                        .and_then(Failure::detail)
                        .map(safe)
                        .unwrap_or_else(|| safe(&item.path.to_string_lossy()))
                } else {
                    safe(&item.path.to_string_lossy())
                }
            })
            .unwrap_or_default()
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(note).style(Style::default().fg(if problem.is_some() {
            colors.error
        } else {
            colors.subtle
        })),
        Rect::new(inner.x, footer.saturating_sub(1), inner.width, 1),
    );
    if !browse
        && items.get(selected).is_some_and(|item| {
            state
                .errors
                .get(&item.id)
                .and_then(Failure::detail)
                .is_some()
        })
    {
        app.hits.push(Hit {
            area: Rect::new(inner.x, footer.saturating_sub(1), inner.width, 1),
            action: Action::Attachment(Command::Details),
        });
    }
    // Release dialog borrows before buttons query the current action guards.
    if browse {
        button(
            frame,
            app,
            Rect::new(inner.x, inner.y, 3, 1),
            " ↑ ",
            Action::Attachment(Command::Parent),
            false,
        );
        button(
            frame,
            app,
            Rect::new(inner.right() - 3, inner.y, 3, 1),
            " → ",
            Action::Attachment(Command::EnterPath),
            false,
        );
    }
    let buttons = controls(app);
    let columns = if inner.width < 46 { 2 } else { 4 };
    for (row, group) in buttons.chunks(columns).enumerate() {
        let labels: Vec<_> = group.iter().map(|c| app.i18n.text(c.label())).collect();
        let total: u16 = labels.iter().map(|s| s.width() as u16 + 2).sum::<u16>()
            + group.len().saturating_sub(1) as u16 * 2;
        let mut x = inner.x + inner.width.saturating_sub(total) / 2;
        for (col, (command, label)) in group.iter().zip(labels).enumerate() {
            let width = (label.width() as u16 + 2).min(inner.right().saturating_sub(x));
            button(
                frame,
                app,
                Rect::new(x, footer + row as u16 * 2, width, 1),
                &label,
                Action::Attachment(command.clone()),
                focus == 2 + row * columns + col,
            );
            x += width + 2;
        }
    }
}
pub fn chips(frame: &mut Frame<'_>, app: &mut App, area: Rect, session: &str) {
    if area.is_empty() {
        return;
    }
    let Some(items) = app.attachments.saved.get(session).filter(|i| !i.is_empty()) else {
        return;
    };
    let item = items
        .iter()
        .find(|item| {
            app.attachments
                .active
                .as_ref()
                .is_some_and(|active| active.ticket.id == item.id)
        })
        .or_else(|| items.iter().find(|item| item.attachment.is_none()))
        .unwrap_or(&items[0]);
    let name = item
        .manifest
        .as_ref()
        .map(|m| m.name.as_str())
        .unwrap_or_else(|| item.path.file_name().and_then(|n| n.to_str()).unwrap_or(""));
    let (status, _) = app.attachments.status(item);
    let extra = if items.len() > 1 {
        format!("  +{}", items.len() - 1)
    } else {
        String::new()
    };
    let icon = if items
        .iter()
        .any(|item| app.attachments.errors.contains_key(&item.id))
    {
        "!"
    } else if app.attachments.ready(session) {
        "✓"
    } else {
        "↑"
    };
    let suffix = format!("{extra} · {}", app.i18n.text(status));
    let name = safe(name);
    let limit = usize::from(area.width).saturating_sub(2 + suffix.width());
    let name = if name.width() > limit {
        let mut cells = 0;
        let mut visible: String = name
            .graphemes(true)
            .take_while(|glyph| {
                cells += glyph.width();
                cells <= limit.saturating_sub(1)
            })
            .collect();
        if limit > 0 {
            visible.push(if app.chrome.ascii { '.' } else { '…' });
        }
        visible
    } else {
        name
    };
    let text = Line::from(vec![
        Span::styled(
            format!("{icon} "),
            Style::default().fg(app.theme.colors().accent),
        ),
        Span::raw(name),
        Span::styled(suffix, Style::default().fg(app.theme.colors().subtle)),
    ]);
    frame.render_widget(Paragraph::new(text), area);
    app.hits.push(Hit {
        area,
        action: Action::Attachment(Command::Open),
    });
}
