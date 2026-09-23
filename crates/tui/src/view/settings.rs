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
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let area = area.inner(Margin::new(1, 0));
    let values = [
        app.theme.title(&app.i18n),
        app.i18n
            .format("language", &[("language", &app.i18n.language_name())]),
        app.i18n.text(if app.chrome.ascii {
            "symbols-ascii"
        } else {
            "symbols-unicode"
        }),
        app.i18n.text(if app.chrome.motion {
            "motion-on"
        } else {
            "motion-off"
        }),
        app.i18n.text("route-connections"),
        app.i18n.text(if app.theme.busy() {
            "theme-loading"
        } else {
            "theme-customize"
        }),
    ];
    for (index, (action, value)) in app.page_actions().into_iter().zip(values).enumerate() {
        let rect =
            Rect::new(area.x, area.y + index as u16 * 2, area.width.min(64), 1).intersection(area);
        let glyph = icon(app, &action);
        let title = format!(
            "{glyph}{}{value}",
            " ".repeat(4usize.saturating_sub(glyph.width()))
        );
        list_item(
            frame,
            app,
            rect,
            &title,
            action,
            app.focus == Focus::Page && app.selected_control == index,
        );
    }
    let mut note = Vec::new();
    if let Some(error) = app.theme.error_text(&app.i18n) {
        note.push(Line::styled(
            error,
            Style::default().fg(app.theme.colors().warning),
        ));
    }
    if app.selected_control == 5
        || app.theme.choice == crate::theme::Choice::Custom
        || app.theme.error.is_some()
    {
        if let Some(path) = &app.theme.path {
            note.push(Line::raw(app.i18n.format(
                "theme-path",
                &[("path", &safe(&path.to_string_lossy()))],
            )));
        }
        note.push(Line::raw(app.i18n.text("theme-file-hint")));
    }
    let diagnostics = app.i18n.diagnostics();
    if !diagnostics.is_empty() {
        note.push(Line::raw(app.i18n.format(
            "localization-errors",
            &[("count", &diagnostics.len().to_string())],
        )));
        note.extend(diagnostics.into_iter().map(Line::raw));
    }
    let footer = Rect::new(
        area.x,
        area.y + 12,
        area.width.min(64),
        area.height.saturating_sub(12),
    )
    .intersection(area);
    frame.render_widget(
        Paragraph::new(note)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(app.theme.colors().subtle)),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn settings_labels_share_a_column_for_cjk_unicode_and_ascii_icons() {
        for (locale, initials) in [
            (crate::Locale::En, ["P", "L", "I", "M", "M", "C"]),
            (crate::Locale::ZhCn, ["配", "语", "图", "动", "模", "自"]),
            (crate::Locale::ZhTw, ["配", "語", "圖", "動", "模", "自"]),
        ] {
            for ascii in [false, true] {
                let mut app = App::new(
                    "/unused".into(),
                    crate::i18n::I18n::new(crate::LocalePreference::Explicit(locale), locale),
                );
                app.apply(Action::Visit(Route::Settings));
                app.chrome.ascii = ascii;
                let mut screen = Terminal::new(TestBackend::new(40, 15)).unwrap();
                screen
                    .draw(|frame| draw(frame, &mut app, frame.area()))
                    .unwrap();
                for (row, initial) in initials.into_iter().enumerate() {
                    assert_eq!(
                        screen.backend().buffer()[(5, row as u16 * 2)].symbol(),
                        initial
                    );
                }
            }
        }
    }
}
