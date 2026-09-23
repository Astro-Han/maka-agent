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

use super::{Command, Entity, Kind};
use crate::app::{Action, App};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::Style,
    text::Line,
    widgets::{Block, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// Localized prose: keep Latin words together without treating a whole CJK
// sentence as one word. Measure and draw these same lines, with no second wrap.
pub(crate) fn note_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();
    for source in text.lines() {
        let mut line = String::new();
        let mut cells = 0;
        // Keep closing punctuation with its preceding word, including CJK words.
        let closing =
            |c: char| ",.!?:;%)]}、。，．！？：；％）］｝〉》」』】〕〗〙〛’”»".contains(c);
        let mut words: Vec<String> = Vec::new();
        for word in source.split_word_bounds() {
            if word.chars().all(closing)
                && let Some(previous) = words.last_mut()
            {
                previous.push_str(word);
            } else {
                words.push(word.into());
            }
        }
        for word in words {
            if cells > 0 && cells + word.width() > width {
                lines.push(Line::from(std::mem::take(&mut line)));
                cells = 0;
            }
            if cells == 0 && word.trim().is_empty() {
                continue;
            }
            for grapheme in word.graphemes(true) {
                if cells > 0 && cells + grapheme.width() > width {
                    // An overlong word may still split; carry its last grapheme
                    // rather than starting the next line with punctuation alone.
                    let carry = grapheme
                        .chars()
                        .all(closing)
                        .then(|| line.grapheme_indices(true).next_back().map(|(i, _)| i))
                        .flatten()
                        .filter(|i| *i > 0 && line[*i..].width() + grapheme.width() <= width);
                    if let Some(index) = carry {
                        let tail = line.split_off(index);
                        lines.push(Line::from(std::mem::replace(&mut line, tail)));
                        cells = line.width();
                    } else {
                        lines.push(Line::from(std::mem::take(&mut line)));
                        cells = 0;
                    }
                }
                line.push_str(grapheme);
                cells += grapheme.width();
            }
        }
        lines.push(Line::from(line));
    }
    lines
}

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect, base: Style) {
    if app
        .management
        .dialog
        .as_ref()
        .is_some_and(|d| d.kind == Kind::Remove)
    {
        app.hits.clear();
        super::removal::draw(frame, app, area, base);
        return;
    }
    if app
        .management
        .dialog
        .as_ref()
        .is_some_and(|dialog| dialog.kind == Kind::Oauth)
    {
        super::oauth::draw(frame, app, area, base);
        return;
    }
    app.hits.clear();
    if app
        .management
        .dialog
        .as_ref()
        .is_some_and(|d| d.enabled_models.is_some())
    {
        super::enabled_models::draw(frame, app, area, base);
        return;
    }
    if app
        .management
        .dialog
        .as_ref()
        .is_some_and(|d| d.credentials.is_some())
    {
        super::credentials::draw(frame, app, area, base);
        return;
    }
    if app
        .management
        .dialog
        .as_ref()
        .is_some_and(|d| d.models.is_some())
    {
        super::models::draw(frame, app, area, base);
        return;
    }
    if app
        .management
        .dialog
        .as_ref()
        .is_some_and(|d| d.locations.is_some())
    {
        super::locations::draw(frame, app, area, base);
        return;
    }
    if app
        .management
        .dialog
        .as_ref()
        .is_some_and(|d| d.chooser.is_some())
    {
        super::choose_project::draw(frame, app, area, base);
        return;
    }
    if app
        .management
        .dialog
        .as_ref()
        .is_some_and(|d| d.browser.is_some())
    {
        super::directory::draw(frame, app, area, base);
        return;
    }
    let busy = app.management.pending.is_some();
    let Some(dialog) = &mut app.management.dialog else {
        return;
    };
    let width = area.width.saturating_sub(2).min(64);
    let editing = dialog.kind.edits_text() && !dialog.reviewing;
    let workspace = dialog.kind.edits_path();
    let endpoint_review = dialog.kind.edits_endpoint() && dialog.reviewing;
    let endpoint_lines =
        endpoint_review.then(|| note_lines(dialog.editor.text(), width.saturating_sub(4)));
    let field_height = if let Some(lines) = &endpoint_lines {
        lines.len() as u16
    } else if workspace || dialog.kind.edits_endpoint() {
        dialog.editor.preferred_height(
            width.saturating_sub(4),
            if area.height >= 14 { 3 } else { 1 },
        )
    } else {
        1
    };
    let name_rows = u16::from(
        matches!(dialog.kind, Kind::Workspace | Kind::Relink) || dialog.kind.edits_endpoint(),
    );
    let text = if busy {
        Some(
            if dialog.kind == Kind::Connection(super::connection::Change::Test) {
                "connection-test-working"
            } else {
                "session-saving"
            },
        )
    } else {
        dialog.editor.error.or(dialog.error).or(match dialog.kind {
            Kind::Connection(change) => {
                Some(if dialog.kind.edits_endpoint() && !dialog.reviewing {
                    "connection-endpoint-edit-note"
                } else {
                    change.note()
                })
            }
            Kind::Rename => None,
            Kind::Reference
            | Kind::Oauth
            | Kind::Project
            | Kind::Locations
            | Kind::Model
            | Kind::Remove
            | Kind::Credential(_) => {
                unreachable!("readers drawn separately")
            }
            Kind::Register => Some("project-register-note"),
            Kind::Relink => Some(if dialog.reviewing {
                "project-relink-note"
            } else {
                "project-relink-path-note"
            }),
            Kind::Workspace
                if matches!(
                    dialog.target.entity,
                    Entity::Session {
                        project_bound: true,
                        ..
                    }
                ) =>
            {
                Some("session-workspace-project-note")
            }
            Kind::Workspace => Some("session-workspace-note"),
            Kind::Archive | Kind::Restore => {
                Some(if matches!(dialog.target.entity, Entity::Project { .. }) {
                    "project-archive-note"
                } else {
                    "session-archive-note"
                })
            }
        })
    }
    .map(|key| app.i18n.text(key));
    let text = if !busy && dialog.error.is_none() {
        dialog
            .connection_test
            .as_ref()
            .map(|test| super::connection_test::text(test, &app.i18n))
            .or(text)
    } else {
        text
    };
    let content = text
        .as_ref()
        .map(|text| note_lines(text, width.saturating_sub(4)));
    let lines = content.as_ref().map_or(0, Vec::len);
    let required_height = 6 + name_rows + field_height + lines as u16;
    if required_height > area.height.saturating_sub(2) {
        dialog.visible = false;
        dialog.editor.invalidate_geometry();
        crate::view::clear_overlay(frame, area);
        frame.render_widget(
            Paragraph::new(app.i18n.text("terminal-small")).wrap(Wrap { trim: false }),
            area.inner(Margin::new(1, 1)),
        );
        return;
    }
    let height = area.height.saturating_sub(2).min(required_height);
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
        .title(app.i18n.text(if endpoint_review {
            "connection-endpoint-confirm"
        } else if dialog.reviewing {
            "project-relink-confirm"
        } else {
            dialog.kind.label(&dialog.target)
        }))
        .style(base)
        .border_style(Style::default().fg(app.theme.colors().subtle));
    let inner = block.inner(popup).inner(Margin::new(1, 0));
    app.modal_area = Some(popup);
    crate::view::clear_overlay(frame, popup);
    frame.render_widget(block, popup);
    dialog.visible = true;
    if name_rows > 0 {
        frame.render_widget(
            Paragraph::new(crate::view::safe(&dialog.target.name)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
    }
    let name_area = Rect::new(inner.x, inner.y + 1 + name_rows, inner.width, field_height);
    if let Some(lines) = endpoint_lines {
        dialog.editor.invalidate_geometry();
        frame.render_widget(Paragraph::new(lines), name_area);
    } else if dialog.kind.edits_text() {
        frame.render_widget(
            Block::default().style(Style::default().bg(app.theme.colors().surface)),
            name_area,
        );
        dialog.editor.draw(
            frame,
            name_area,
            editing && dialog.focus == 0 && !busy && !dialog.blocked,
            app.theme.colors(),
        );
    } else {
        frame.render_widget(
            Paragraph::new(crate::view::safe(&dialog.target.name)),
            name_area,
        );
    }
    if let Some(content) = content {
        frame.render_widget(
            Paragraph::new(content).style(Style::default().fg(
                if dialog.error.is_some() || dialog.editor.error.is_some() || matches!(dialog.connection_test, Some(maka_protocol::connection_effects::ConnectionTestProjection::Failed {..})) {
                    app.theme.colors().warning
                } else if dialog.connection_test.is_some() {
                    crate::view::tone::accent(app.theme.colors())
                } else {
                    app.theme.colors().subtle
                },
            )),
            Rect::new(
                inner.x,
                name_area.bottom() + 1,
                inner.width,
                inner.bottom().saturating_sub(name_area.bottom() + 2),
            ),
        );
    }
    let focus = dialog.focus;
    if dialog.connection_test.is_some() {
        let text = app.i18n.text("connection-test-close");
        let width = (text.width() as u16 + 2).min(inner.width);
        crate::view::button(
            frame,
            app,
            Rect::new(inner.right() - width, inner.bottom() - 1, width, 1),
            &text,
            Action::Manage(Command::Close),
            true,
        );
        return;
    }
    let kind = dialog.kind;
    let reviewing = dialog.reviewing;
    let buttons_start = usize::from(editing) + usize::from(kind == Kind::Register);
    let save_label = if kind.edits_endpoint() {
        if reviewing {
            "connection-endpoint-apply"
        } else {
            "connection-endpoint-review"
        }
    } else if kind == Kind::Relink {
        if reviewing {
            "project-relink-apply"
        } else {
            "project-relink-review"
        }
    } else if kind == Kind::Register {
        "project-register-apply"
    } else if workspace {
        "session-workspace-apply"
    } else if editing {
        "session-save"
    } else {
        kind.label(&dialog.target)
    };
    let save_width =
        (unicode_width::UnicodeWidthStr::width(app.i18n.text(save_label).as_str()) as u16 + 2)
            .min(inner.width / 2);
    let cancel_width =
        (unicode_width::UnicodeWidthStr::width(app.i18n.text("session-cancel").as_str()) as u16
            + 2)
        .min(inner.width / 2);
    let save = Rect::new(
        inner.right() - save_width,
        inner.bottom() - 1,
        save_width,
        1,
    );
    let edit_label = if kind.edits_endpoint() {
        "connection-endpoint-edit"
    } else {
        "project-relink-edit"
    };
    let edit_width = if reviewing {
        (app.i18n.text(edit_label).width() as u16 + 2).min(inner.width / 3)
    } else {
        0
    };
    let cancel = Rect::new(
        save.x
            .saturating_sub(cancel_width + 2 + edit_width)
            .max(inner.x),
        save.y,
        cancel_width,
        1,
    );
    for (rect, label, command, focused) in [
        (
            cancel,
            "session-cancel",
            Command::Close,
            focus == buttons_start,
        ),
        (
            save,
            save_label,
            Command::Save,
            focus == if reviewing { 2 } else { buttons_start + 1 },
        ),
    ] {
        crate::view::button(
            frame,
            app,
            rect,
            &app.i18n.text(label),
            Action::Manage(command),
            focused,
        );
    }
    if reviewing {
        crate::view::button(
            frame,
            app,
            Rect::new(save.x.saturating_sub(edit_width + 1), save.y, edit_width, 1),
            &app.i18n.text(edit_label),
            Action::Manage(Command::Edit),
            focus == 1,
        );
    }
    if kind == Kind::Register {
        crate::view::button(
            frame,
            app,
            Rect::new(inner.x, save.y, cancel.x.saturating_sub(inner.x + 1), 1),
            &app.i18n.text("directory-browse"),
            Action::Manage(Command::Browse),
            focus == 1,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notes_wrap_latin_words_and_cjk_without_losing_unicode() {
        let source =
            "Host 上的目录。 Existing sessions and files remain. abcdefghijklmn e\u{301}🦀";
        let lines = note_lines(source, 12);
        let texts: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(texts.iter().all(|line| line.width() <= 12));
        let compact = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        assert_eq!(compact(&texts.concat()), compact(source));
        assert!(texts.iter().any(|s| s.contains("Existing")));
        assert!(texts.iter().any(|s| s.contains("sessions")));
        assert!(
            texts[0].contains("Host 上"),
            "CJK must share the line with the Latin prefix"
        );
        for width in [4, 8, 12, 44] {
            let source = "后续请求使用新密钥。保存不会测试。 Words, words. abcdefghijklmnop。";
            let lines = note_lines(source, width);
            let texts: Vec<String> = lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();
            assert!(texts.iter().all(|s| s.width() <= usize::from(width)));
            assert!(texts.iter().all(|s| !s.starts_with(['。', ',', '.'])));
            assert_eq!(compact(&texts.concat()), compact(source));
        }
    }
}
