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

use crate::{
    app::{Action, App},
    i18n::I18n,
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Default)]
pub struct State {
    items: Vec<(Action, &'static str)>,
    query: String,
    list: Option<Rect>,
    top: usize,
    dragging: bool,
}

impl State {
    pub fn new(items: Vec<(Action, &'static str)>) -> Self {
        Self {
            items,
            ..Self::default()
        }
    }
    pub fn filtered(&self, i18n: &I18n) -> Vec<(Action, &'static str)> {
        let query = self.query.to_lowercase();
        self.items
            .iter()
            .filter(|(_, key)| {
                let text = i18n.text(key).to_lowercase();
                query.split_whitespace().all(|word| text.contains(word))
            })
            .cloned()
            .collect()
    }
    pub fn invalidate(&mut self) {
        self.list = None;
        self.dragging = false;
    }
    fn insert(&mut self, text: &str) {
        for ch in text.chars().filter(|ch| !ch.is_control()) {
            if self.query.len() + ch.len_utf8() > 512 {
                break;
            }
            self.query.push(ch);
        }
    }
}

impl App {
    pub(crate) fn palette_input(&mut self, event: Event) -> (bool, Option<Action>) {
        let items = self.commands();
        let selected = self.palette.unwrap_or_default();
        let last = items.len().saturating_sub(1);
        let mut action = None;
        let mut changed_query = false;
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.command_palette.dragging = false;
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
                    return (true, Some(Action::Quit));
                }
                match key.code {
                    KeyCode::Esc => self.palette = None,
                    KeyCode::Up => self.palette = Some(selected.saturating_sub(1)),
                    KeyCode::Down => self.palette = Some((selected + 1).min(last)),
                    KeyCode::Home => self.palette = Some(0),
                    KeyCode::End => self.palette = Some(last),
                    KeyCode::PageUp | KeyCode::PageDown => {
                        let page = self
                            .command_palette
                            .list
                            .map_or(1, |r| usize::from(r.height));
                        self.palette = Some(if key.code == KeyCode::PageUp {
                            selected.saturating_sub(page)
                        } else {
                            (selected + page).min(last)
                        });
                    }
                    KeyCode::Enter if self.command_palette.list.is_some() => {
                        action = items.get(selected).map(|(action, _)| action.clone())
                    }
                    KeyCode::Backspace => {
                        if let Some((index, _)) = self
                            .command_palette
                            .query
                            .grapheme_indices(true)
                            .next_back()
                        {
                            self.command_palette.query.truncate(index);
                            changed_query = true;
                        }
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.command_palette.query.clear();
                        changed_query = true;
                    }
                    KeyCode::Char(ch)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        self.command_palette.insert(&ch.to_string());
                        changed_query = true;
                    }
                    _ => return (false, None),
                }
            }
            Event::Paste(text) => {
                self.command_palette.insert(&text);
                changed_query = true;
            }
            Event::Mouse(mouse) => {
                let state = &mut self.command_palette;
                if let Some(list) = state.list {
                    match mouse.kind {
                        MouseEventKind::Up(MouseButton::Left) => state.dragging = false,
                        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
                            if list.contains((mouse.column, mouse.row).into()) =>
                        {
                            state.dragging = false;
                            let capacity = usize::from(list.height).max(1);
                            state.top = if mouse.kind == MouseEventKind::ScrollDown {
                                (state.top + 3).min(items.len().saturating_sub(capacity))
                            } else {
                                state.top.saturating_sub(3)
                            };
                            self.palette = Some(
                                selected.clamp(state.top, (state.top + capacity - 1).min(last)),
                            );
                            self.hover = None;
                            self.hover_area = None;
                        }
                        MouseEventKind::Down(MouseButton::Left)
                        | MouseEventKind::Drag(MouseButton::Left)
                            if state.dragging
                                || (items.len() > usize::from(list.height)
                                    && mouse.column == list.right() - 1
                                    && list.contains((mouse.column, mouse.row).into())) =>
                        {
                            state.dragging = true;
                            let position = usize::from(
                                mouse
                                    .row
                                    .saturating_sub(list.y)
                                    .min(list.height.saturating_sub(1)),
                            );
                            state.top = position
                                * items.len().saturating_sub(usize::from(list.height))
                                / usize::from(list.height.saturating_sub(1).max(1));
                            self.palette = Some(state.top);
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            action = self
                                .hits
                                .iter()
                                .find(|h| h.area.contains((mouse.column, mouse.row).into()))
                                .map(|h| h.action.clone());
                        }
                        _ => return (false, None),
                    }
                } else {
                    return (false, None);
                }
            }
            _ => return (false, None),
        }
        if changed_query {
            self.palette = Some(0);
            self.command_palette.top = 0;
            // Query edits retire row geometry before another click can arrive.
            self.command_palette.invalidate();
            self.hits.clear();
        }
        if action.as_ref().is_some_and(|action| self.enabled(action)) {
            self.palette = None;
            self.hits.clear();
            return (true, action.and_then(|action| self.apply(action)));
        }
        (true, None)
    }
}

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    app.hits.clear();
    if area.width < 24 || area.height < 8 {
        app.command_palette.invalidate();
        return;
    }
    let width = area.width.saturating_sub(4).min(64);
    let items = app.commands();
    // Shrink only the results below the anchored search field.
    let maximum_height = (app.command_palette.items.len() as u16 + 6)
        .min(26)
        .min(area.height - 2);
    let height = (items.len().max(1) as u16 + 6).min(maximum_height);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - maximum_height) / 2,
        width,
        height,
    );
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    let colors = app.theme.colors();
    let block = Block::bordered()
        .border_type(if app.chrome.ascii {
            ratatui::widgets::BorderType::Plain
        } else {
            ratatui::widgets::BorderType::Rounded
        })
        .title(app.i18n.text("palette-title"))
        .title_alignment(Alignment::Center)
        .style(base)
        .border_style(Style::default().fg(colors.accent));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let search = Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), 1);
    let query = &app.command_palette.query;
    let label = if query.is_empty() {
        app.i18n.text("palette-filter")
    } else {
        query.clone()
    };
    let offset = query
        .width()
        .saturating_sub(usize::from(search.width.saturating_sub(1)));
    frame.render_widget(
        Paragraph::new(label)
            .style(Style::default().fg(if query.is_empty() {
                colors.subtle
            } else {
                colors.foreground
            }))
            .scroll((0, offset as u16)),
        search,
    );
    frame.set_cursor_position((
        search.x + query.width().saturating_sub(offset) as u16,
        search.y,
    ));
    let list = Rect::new(
        inner.x + 1,
        inner.y + 3,
        inner.width.saturating_sub(2),
        inner.height.saturating_sub(3),
    );
    let selected = app
        .palette
        .unwrap_or_default()
        .min(items.len().saturating_sub(1));
    app.palette = Some(selected);
    let capacity = usize::from(list.height).max(1);
    let state = &mut app.command_palette;
    state.list = Some(list);
    state.top = state
        .top
        .min(selected)
        .max(selected.saturating_sub(capacity - 1))
        .min(items.len().saturating_sub(capacity));
    let top = state.top;
    let scrolling = items.len() > capacity;
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(app.i18n.text("palette-empty"))
                .style(Style::default().fg(colors.subtle)),
            list,
        );
    }
    for (index, (action, key)) in items.iter().enumerate().skip(top).take(capacity) {
        let row = Rect::new(
            list.x,
            list.y + (index - top) as u16,
            list.width.saturating_sub(if scrolling { 2 } else { 0 }),
            1,
        );
        crate::view::list_item(
            frame,
            app,
            row,
            &app.i18n.text(key),
            action.clone(),
            index == selected,
        );
    }
    if scrolling {
        let widget = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some(app.chrome.symbol("│", "|")))
            .thumb_symbol(app.chrome.symbol("█", "#"))
            .track_style(Style::default().fg(colors.subtle))
            .thumb_style(Style::default().fg(colors.accent));
        let mut state = ScrollbarState::new(items.len() - capacity + 1)
            .position(top)
            .viewport_content_length(capacity);
        frame.render_stateful_widget(widget, list, &mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        i18n::{Locale, LocalePreference},
        navigation::Route,
    };
    use crossterm::event::{KeyEvent, MouseEvent};
    use ratatui::{Terminal, backend::TestBackend};
    fn frame(app: &mut App, width: u16, height: u16) {
        Terminal::new(TestBackend::new(width, height))
            .unwrap()
            .draw(|f| crate::view::draw(f, app))
            .unwrap();
    }
    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }
    #[test]
    fn search_and_drag_preserve_captured_commands_drafts_and_published_geometry() {
        for locale in [Locale::En, Locale::ZhCn, Locale::ZhTw] {
            let mut app = App::new(
                "/unused".into(),
                I18n::new(LocalePreference::Explicit(locale), locale),
            );
            app.apply(Action::Visit(Route::Session("draft".into())));
            app.drafts
                .get_mut("draft")
                .unwrap()
                .insert("preserved draft");
            app.apply(Action::Palette);
            frame(&mut app, 52, 18);
            let list = app.command_palette.list.unwrap();
            let selected = app.palette;
            assert_eq!(
                app.input(mouse(MouseEventKind::Moved, list.x, list.y)),
                (true, None)
            );
            assert_eq!(
                app.palette, selected,
                "hover highlights without changing the keyboard choice"
            );
            assert!(app.hover.is_some());
            assert!(
                !app.input(mouse(MouseEventKind::Moved, list.x, list.y)).0,
                "unchanged hover does not redraw"
            );
            let count = app.commands().len();
            assert!(count > usize::from(list.height));
            app.input(mouse(MouseEventKind::ScrollDown, list.x, list.y));
            frame(&mut app, 52, 18);
            assert_eq!(
                app.command_palette.top, 3,
                "wheel scrolls immediately, not after selection reaches the bottom"
            );
            assert!(app.hover.is_none());
            for _ in 0..count {
                if app.hits.iter().any(|hit| hit.action == Action::Quit) {
                    break;
                }
                app.input(mouse(MouseEventKind::ScrollDown, list.x, list.y));
                frame(&mut app, 52, 18);
            }
            let quit = app
                .hits
                .iter()
                .find(|hit| hit.action == Action::Quit)
                .expect("quit is reachable by scrolling")
                .area;
            assert_eq!(
                app.input(mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    quit.x,
                    quit.y
                ))
                .1,
                Some(Action::Quit)
            );
            assert_eq!(app.drafts["draft"].text(), "preserved draft");
            app.apply(Action::Palette);
            frame(&mut app, 52, 18);
            assert!(
                app.input(mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    list.right() - 1,
                    list.bottom() - 1
                ))
                .1
                .is_none()
            );
            frame(&mut app, 52, 18);
            assert_eq!(app.command_palette.top, count - usize::from(list.height));
            app.input(Event::Resize(80, 28));
            app.input(mouse(
                MouseEventKind::Drag(MouseButton::Left),
                list.right() - 1,
                list.y,
            ));
            assert!(app.command_palette.list.is_none());
            frame(&mut app, 80, 28);
            let stale = app.command_palette.list.unwrap();
            app.input(key(KeyCode::Home));
            assert_eq!(app.palette, Some(0));
            app.input(key(KeyCode::End));
            assert_eq!(app.palette, Some(count - 1));
            app.input(Event::Paste("not-a-command 👨‍👩‍👧‍👦".into()));
            app.input(key(KeyCode::Backspace));
            assert_eq!(app.command_palette.query, "not-a-command ");
            frame(&mut app, 80, 28);
            assert!(app.commands().is_empty());
            assert!(app.input(key(KeyCode::Enter)).1.is_none());
            assert!(app.palette.is_some());
            app.input(Event::Key(KeyEvent::new(
                KeyCode::Char('u'),
                KeyModifiers::CONTROL,
            )));
            let query = app.i18n.text("command-settings").to_lowercase();
            app.input(Event::Paste(query));
            assert_eq!(
                app.commands(),
                vec![(Action::Visit(Route::Settings), "command-settings")]
            );
            // Old geometry cannot activate a different row after filtering.
            app.input(mouse(
                MouseEventKind::Down(MouseButton::Left),
                stale.x,
                stale.y,
            ));
            assert_eq!(app.navigation.current(), Route::Session("draft".into()));
            frame(&mut app, 80, 28);
            assert_eq!(app.command_palette.list.unwrap().y, stale.y);
            assert_eq!(app.modal_area.unwrap().height, 7);
            let stable = app.commands();
            app.connection = crate::app::ConnectionState::Failed("offline".into());
            assert_eq!(app.commands(), stable);
            let hit = app
                .hits
                .iter()
                .find(|h| h.action == Action::Visit(Route::Settings))
                .unwrap()
                .area;
            app.input(mouse(
                MouseEventKind::Down(MouseButton::Left),
                hit.x + 2,
                hit.y,
            ));
            assert!(app.palette.is_none());
            assert_eq!(app.navigation.current(), Route::Settings);
            assert_eq!(app.drafts["draft"].text(), "preserved draft");
            app.apply(Action::Palette);
            assert!(app.command_palette.query.is_empty());
            app.input(Event::Paste(app.i18n.text("command-quit")));
            frame(&mut app, 52, 18);
            assert_eq!(app.commands(), vec![(Action::Quit, "command-quit")]);
            assert_eq!(app.input(key(KeyCode::Enter)).1, Some(Action::Quit));
            app.apply(Action::Palette);
            app.input(Event::Paste("🦀".repeat(200)));
            assert!(app.command_palette.query.len() <= 512);
            frame(&mut app, 20, 6);
            assert!(app.input(key(KeyCode::Enter)).1.is_none());
            app.input(key(KeyCode::Esc));
            assert!(app.palette.is_none());
        }
    }
}
