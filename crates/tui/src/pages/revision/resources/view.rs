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

use super::Resource;
use crate::{
    app::{Action, App, Hit},
    pages::revision::Command,
    view::safe,
};
use crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

pub(in crate::pages::revision) fn input(app: &mut App, event: &Event) -> Option<Option<Command>> {
    let state = &mut app.revision;
    if !state.rendered
        || !state.resources.visible
        || state.show_problem
        || state.confirm_discard
        || state.phase != super::super::Phase::Editing
    {
        return None;
    }
    let resources = state.saved.as_ref()?.inputs[state.selected].resources();
    let browser = &mut state.resources;
    let area = browser.area?;
    let capacity = usize::from(area.height / 3).max(1);
    let last = resources.len().saturating_sub(1);
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release && state.focus == 0 => {
            match key.code {
                KeyCode::Up => browser.selected = browser.selected.saturating_sub(1),
                KeyCode::Down => browser.selected = (browser.selected + 1).min(last),
                KeyCode::PageUp => browser.selected = browser.selected.saturating_sub(capacity),
                KeyCode::PageDown => browser.selected = (browser.selected + capacity).min(last),
                KeyCode::Home => browser.selected = 0,
                KeyCode::End => browser.selected = last,
                KeyCode::Char(' ') | KeyCode::Enter => {
                    return Some(
                        resources
                            .get(browser.selected)
                            .cloned()
                            .map(Command::ToggleResource),
                    );
                }
                _ => return None,
            }
        }
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if area.contains((mouse.column, mouse.row).into()) =>
            {
                state.focus = 0;
                browser.selected = if mouse.kind == MouseEventKind::ScrollUp {
                    browser.selected.saturating_sub(2)
                } else {
                    (browser.selected + 2).min(last)
                };
            }
            MouseEventKind::Down(MouseButton::Left)
                if resources.len() > capacity
                    && mouse.column == area.right() - 1
                    && area.contains((mouse.column, mouse.row).into()) =>
            {
                browser.dragging = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if browser.dragging => {}
            MouseEventKind::Up(MouseButton::Left) if browser.dragging => {
                browser.dragging = false;
                return Some(None);
            }
            _ => return None,
        },
        _ => return None,
    }
    if let Event::Mouse(mouse) = event
        && browser.dragging
    {
        let fraction = usize::from(
            mouse
                .row
                .saturating_sub(area.y)
                .min(area.height.saturating_sub(1)),
        );
        browser.top = fraction * resources.len().saturating_sub(capacity)
            / usize::from(area.height.saturating_sub(1).max(1));
        browser.selected = browser.top;
        state.focus = 0;
    }
    Some(None)
}

pub(in crate::pages::revision) fn draw(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    enabled: bool,
) {
    let state = &mut app.revision;
    let input = &state.saved.as_ref().unwrap().inputs[state.selected];
    let resources = input.resources();
    let browser = &mut state.resources;
    browser.area = enabled.then_some(area);
    browser.selected = browser.selected.min(resources.len().saturating_sub(1));
    let capacity = usize::from(area.height / 3).max(1);
    browser.top = browser
        .top
        .min(browser.selected)
        .max(browser.selected.saturating_sub(capacity - 1))
        .min(resources.len().saturating_sub(capacity));
    let colors = app.theme.colors();
    for (index, resource) in resources
        .iter()
        .enumerate()
        .skip(browser.top)
        .take(capacity)
    {
        let included = input.included(resource);
        let immutable = matches!(resource, Resource::Inline { .. });
        let (title, detail) = match resource {
            Resource::Attachment { index } => {
                let file = &input.original.content.attachments.as_ref().unwrap()[*index];
                let size = if file.bytes < 1024 {
                    format!("{} B", file.bytes)
                } else if file.bytes < 1024 * 1024 {
                    format!("{:.1} KiB", file.bytes as f64 / 1024.0)
                } else {
                    format!("{:.1} MiB", file.bytes as f64 / (1024.0 * 1024.0))
                };
                let mut detail = format!("{} · {size}", file.mime_type);
                match &file.storage_ref {
                    maka_protocol::turn::StorageRef::WorkspaceFile { relative_path } => {
                        detail.push_str(&format!(" · {relative_path}"))
                    }
                    maka_protocol::turn::StorageRef::ExternalFile { absolute_path } => {
                        detail.push_str(&format!(" · {absolute_path}"))
                    }
                    _ => {}
                }
                (file.name.clone(), detail)
            }
            Resource::Quote { index } => {
                let quote = &input.original.content.quotes.as_ref().unwrap()[*index];
                (
                    quote
                        .label
                        .clone()
                        .unwrap_or_else(|| app.i18n.text("revision-resource-quote")),
                    quote.text.clone(),
                )
            }
            Resource::Directory { index } => {
                let directory = &input
                    .original
                    .content
                    .directory_references
                    .as_ref()
                    .unwrap()[*index];
                (directory.path.clone(), directory.host_id.clone())
            }
            Resource::Selection { provider, index } => (
                input.original.input_selections[provider][*index].clone(),
                provider.clone(),
            ),
            Resource::Inline { index } => {
                let reference = &input.content.inline_references.as_ref().unwrap()[*index];
                (
                    reference.label.clone(),
                    app.i18n.text("revision-resource-inline"),
                )
            }
        };
        let rect = Rect::new(
            area.x,
            area.y + ((index - browser.top) * 3) as u16,
            area.width.saturating_sub(2),
            2.min(area.height),
        );
        let selected = enabled && state.focus == 0 && browser.selected == index;
        let base = if selected {
            colors.selected()
        } else {
            colors.base()
        };
        let mark = if immutable {
            "·"
        } else if included {
            app.chrome.symbol("✓", "x")
        } else {
            " "
        };
        let text = if immutable {
            format!(" {mark}  {}", safe(&title))
        } else {
            format!("[{mark}] {}", safe(&title))
        };
        frame.render_widget(
            Paragraph::new(text).style(base.fg(if included {
                colors.foreground
            } else {
                colors.muted
            })),
            Rect::new(rect.x, rect.y, rect.width, 1),
        );
        if area.height > 1 {
            frame.render_widget(
                Paragraph::new(format!("    {}", safe(&detail))).style(base.fg(colors.muted)),
                Rect::new(rect.x, rect.y + 1, rect.width, 1),
            );
        }
        if enabled && !immutable {
            app.hits.push(Hit {
                area: rect,
                action: Action::Revision(Command::ToggleResource(resource.clone())),
            });
        }
    }
    if resources.len() > capacity {
        let mut scroll = ScrollbarState::new(resources.len())
            .position(browser.top)
            .viewport_content_length(capacity);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(Style::default().fg(colors.subtle))
                .thumb_style(Style::default().fg(colors.accent)),
            area,
            &mut scroll,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::revision::{
        Output,
        tests::{frame, sources},
    };
    use crossterm::event::{KeyEvent, KeyModifiers, MouseEvent};
    #[test]
    fn resource_list_scrolls_toggles_and_keeps_edits_without_touching_inline_tokens() {
        let (mut app, basis) = crate::pages::branch::tests::fixture();
        app.apply(Action::Revision(Command::Open(basis)));
        let request = app.revision_request().unwrap();
        let mut source = sources("source");
        source.messages[1].input_selections.insert(
            "skills".into(),
            (0..12).map(|i| format!("skill-{i}")).collect(),
        );
        app.revision_completed(request, Ok(Output::Sources(source)));
        frame(&mut app, 80, 30);
        app.apply(Action::Revision(Command::Resources));
        frame(&mut app, 80, 30);
        assert!(!app.revision_enabled(&Command::ToggleResource(Resource::Inline { index: 0 })));
        app.apply(Action::Revision(Command::Select(1)));
        frame(&mut app, 80, 30);
        app.apply(Action::Revision(Command::Resources));
        frame(&mut app, 80, 30);
        app.input(Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)));
        let screen = frame(&mut app, 80, 30);
        assert!(screen.contains("skill-11"));
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Char(' '),
            KeyModifiers::NONE,
        )));
        assert_eq!(
            app.revision.saved.as_ref().unwrap().inputs[1].excluded,
            [Resource::Selection {
                provider: "skills".into(),
                index: 11
            }]
        );
        let area = app.revision.resources.area.unwrap();
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.input(Event::Mouse(MouseEvent {
                kind,
                column: area.right() - 1,
                row: area.y,
                modifiers: KeyModifiers::NONE,
            }));
        }
        frame(&mut app, 80, 30);
        assert_eq!(app.revision.resources.top, 0);
        let hit = app
            .hits
            .iter()
            .find(|hit| {
                matches!(
                    hit.action,
                    Action::Revision(Command::ToggleResource(Resource::Attachment { index: 0 }))
                )
            })
            .unwrap()
            .area;
        app.input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x + 6,
            row: hit.y + 1,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(
            app.revision.saved.as_ref().unwrap().inputs[1]
                .message()
                .content
                .attachments
                .is_none()
        );
        let saved = app.revision.checkpoint().unwrap();
        saved.validate("root").unwrap();
        app.revision.restore(saved);
        assert!(!app.revision.visible);
        assert!(
            app.revision.saved.as_ref().unwrap().inputs[1]
                .message()
                .content
                .attachments
                .is_none()
        );
        app.apply(Action::Revision(Command::Resume));
        frame(&mut app, 80, 24);
        app.apply(Action::Revision(Command::Resources));
        frame(&mut app, 80, 24);
        app.input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(!app.revision.visible);
        assert!(
            app.revision.saved.as_ref().unwrap().inputs[1]
                .message()
                .content
                .attachments
                .is_none()
        );
    }
}
