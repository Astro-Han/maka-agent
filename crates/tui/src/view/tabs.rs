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

pub(super) fn label(app: &App, id: &str) -> String {
    app.tabs
        .entries
        .iter()
        .find(|tab| tab.id == id)
        .and_then(|tab| tab.name.clone())
        .unwrap_or_else(|| app.i18n.text("route-session"))
}

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if area.is_empty() || app.tabs.entries.is_empty() {
        return;
    }
    let height = crate::navigation::tabs::Tabs::capacity(area);
    let stride = if area.width >= 12 { 2 } else { 1 };
    let target = if app.focus == Focus::Navigation && app.selected_nav >= Route::ALL.len() {
        Some(app.selected_nav - Route::ALL.len())
    } else {
        app.tabs.reveal.take()
    };
    if let Some(target) = target {
        if target < app.tabs.top {
            app.tabs.top = target;
        } else if target >= app.tabs.top + height {
            app.tabs.top = target + 1 - height;
        }
    }
    app.tabs.top = app
        .tabs
        .top
        .min(app.tabs.entries.len().saturating_sub(height));
    app.tabs.area = Some(area);
    let scrollbar = app.tabs.scrollbar();
    let current = app.navigation.current();
    for index in app.tabs.top..(app.tabs.top + height).min(app.tabs.entries.len()) {
        let id = app.tabs.entries[index].id.clone();
        let action = Action::Visit(Route::Session(id.clone()));
        let close = Action::CloseTab(id.clone());
        let active = current == Route::Session(id.clone());
        let focused =
            app.focus == Focus::Navigation && app.selected_nav == Route::ALL.len() + index;
        let hovered = app
            .hover
            .as_ref()
            .is_some_and(|hover| hover == &action || hover == &close);
        let row = Rect::new(
            area.x,
            area.y + (index - app.tabs.top) as u16 * stride,
            area.width.saturating_sub(u16::from(scrollbar.is_some())),
            1,
        );
        let wide = row.width >= 12;
        let closable = wide && (active || focused || hovered);
        let body = Rect::new(
            row.x,
            row.y,
            row.width.saturating_sub(if closable { 2 } else { 0 }),
            1,
        );
        let working = app.session_activity(&id) == super::activity::Activity::Working;
        let text = if wide && working {
            let orbit = app
                .chrome
                .animation
                .frame(crate::motion::Loop::OrbitSmall, app.chrome.ascii);
            format!("{orbit} {}", label(app, &id))
        } else if wide {
            format!(
                "{} {}",
                if active {
                    app.chrome.symbol("›", ">")
                } else {
                    " "
                },
                label(app, &id)
            )
        } else if working {
            app.chrome
                .animation
                .frame(crate::motion::Loop::OrbitSmall, app.chrome.ascii)
                .into()
        } else {
            // Compact rail: ordered destinations, full localized label on hover.
            format!("{}", index + 1)
        };
        let draw_control = if wide { list_item } else { button };
        draw_control(frame, app, body, &text, action, focused);
        if !focused && !hovered {
            frame.buffer_mut().set_style(
                body,
                Style::default().fg(tone::session(&id, app.theme.colors())),
            );
        }
        if active && !wide {
            frame.buffer_mut().set_style(
                body,
                Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            );
        }
        if closable {
            let close_area = Rect::new(body.right(), row.y, 2, 1);
            let symbol = format!(" {}", app.chrome.symbol("×", "x"));
            button(frame, app, close_area, &symbol, close, false);
        }
    }
    if let Some((track, thumb)) = scrollbar {
        for y in track.y..track.bottom() {
            let on_thumb = y >= thumb.y && y < thumb.bottom();
            frame.buffer_mut()[(track.x, y)]
                .set_symbol(if on_thumb {
                    app.chrome.symbol("┃", "#")
                } else {
                    app.chrome.symbol("│", "|")
                })
                .set_fg(if on_thumb {
                    tone::secondary(app.theme.colors())
                } else {
                    app.theme.colors().subtle
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn spaced_tabs_scroll_and_drag_independently_without_switching_or_editing() {
        let mut app = App::new(
            "/fixture".into(),
            crate::i18n::I18n::new(
                crate::LocalePreference::Explicit(crate::Locale::En),
                crate::Locale::En,
            ),
        );
        app.connection = ConnectionState::Connected {
            root_id: "r".into(),
            epoch: "e".into(),
        };
        app.apply(Action::Visit(Route::Session("s0".into())));
        for index in 1..20 {
            app.tabs.open(&format!("s{index}"));
        }
        app.tabs.reveal = Some(0);
        let mut item = crate::pages::sessions::tests::item("s0");
        item.status = maka_protocol::session::SessionStatus::Running;
        app.sessions.items.push(item);
        app.chrome.motion = false;
        app.focus = Focus::Composer;
        app.drafts.get_mut("s0").unwrap().insert("keep 中文🦀");
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let render = |app: &mut App, terminal: &mut Terminal<TestBackend>| {
            terminal.draw(|f| crate::view::draw(f, app)).unwrap();
        };
        render(&mut app, &mut terminal);
        let area = app.tabs.area.unwrap();
        assert_eq!(terminal.backend().buffer()[(area.x, area.y)].symbol(), "⢁");
        let rows: Vec<_> = app
            .hits
            .iter()
            .filter(|h| matches!(h.action, Action::Visit(Route::Session(_))))
            .map(|h| h.area.y)
            .collect();
        assert!(rows.windows(2).all(|rows| rows[1] - rows[0] == 2));
        assert_eq!(
            terminal.backend().buffer()[(area.x, area.y + 1)].symbol(),
            " "
        );
        let mouse = |kind, column, row| {
            Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        app.input(mouse(MouseEventKind::ScrollDown, area.x, area.y));
        render(&mut app, &mut terminal);
        assert_eq!(app.tabs.top, 1);
        let (track, thumb) = app.tabs.scrollbar().unwrap();
        app.input(mouse(
            MouseEventKind::Down(MouseButton::Left),
            thumb.x,
            thumb.y,
        ));
        render(&mut app, &mut terminal); // Animation redraw cannot lose the scrollbar drag.
        app.input(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            track.x,
            track.bottom() - 1,
        ));
        render(&mut app, &mut terminal);
        assert_eq!(
            app.tabs.top,
            20 - crate::navigation::tabs::Tabs::capacity(area)
        );
        app.input(mouse(
            MouseEventKind::Up(MouseButton::Left),
            track.x,
            track.bottom() - 1,
        ));
        assert_eq!(app.navigation.current(), Route::Session("s0".into()));
        assert_eq!(app.focus, Focus::Composer);
        assert_eq!(app.drafts["s0"].text(), "keep 中文🦀");
        app.focus = Focus::Navigation;
        app.selected_nav = Route::ALL.len();
        render(&mut app, &mut terminal);
        assert_eq!(
            app.tabs.top, 0,
            "keyboard focus is revealed after mouse scrolling"
        );
        app.input(Event::Resize(80, 24));
        assert!(app.tabs.area.is_none());
    }
}
