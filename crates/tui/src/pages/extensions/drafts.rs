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
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

pub(super) struct Conflict {
    id: String,
    draft: Value,
    pub mine: Option<bool>,
}
pub(super) struct Review {
    view: TerminalViewProjection,
    page: Page,
    values: BTreeMap<String, Value>,
    pub conflicts: Vec<Conflict>,
}
impl Review {
    pub fn resolved(&self) -> bool {
        self.conflicts.iter().all(|field| field.mine.is_some())
    }
}
fn value(control: &Control) -> Value {
    match control {
        Control::Toggle { value } => Value::Bool(*value),
        Control::Text { value, .. } => Value::String(value.clone()),
    }
}
fn replace(control: &mut Control, draft: &Value) -> Result<(), ()> {
    match (control, draft) {
        (Control::Toggle { value }, Value::Bool(draft)) => *value = *draft,
        (Control::Text { value, .. }, Value::String(draft)) => *value = draft.clone(),
        _ => return Err(()),
    }
    Ok(())
}
impl State {
    fn merge(&self, view: TerminalViewProjection, page: Page) -> Result<Review, ()> {
        let original = self.page.as_ref().ok_or(())?;
        let mut review = Review {
            view,
            values: page
                .fields
                .iter()
                .map(|field| (field.id.clone(), value(&field.control)))
                .collect(),
            page,
            conflicts: vec![],
        };
        let mut all_drafts = review.page.clone();
        for old in &original.fields {
            let draft = self.drafts.get(&old.id).ok_or(())?;
            if draft == &value(&old.control) {
                continue;
            }
            // Never silently discard a changed field after removal, disablement,
            // a control-type change, or a tighter input constraint.
            let field = all_drafts
                .fields
                .iter_mut()
                .find(|field| field.id == old.id && field.enabled)
                .ok_or(())?;
            let current = value(&field.control);
            replace(&mut field.control, draft)?;
            if current == value(&old.control) || current == *draft {
                review.values.insert(old.id.clone(), draft.clone());
            } else {
                review.conflicts.push(Conflict {
                    id: old.id.clone(),
                    draft: draft.clone(),
                    mine: None,
                });
            }
        }
        all_drafts.validate().map_err(|_| ())?;
        Ok(review)
    }
    pub fn reload_draft(&mut self, view: TerminalViewProjection, page: Page) {
        self.blocked = true;
        let Ok(review) = self.merge(view, page) else {
            self.message = Some(Message::Local("extensions-draft-shape-changed"));
            return;
        };
        let context_changed = self
            .page
            .as_ref()
            .is_some_and(|old| old.title != review.page.title || old.body != review.page.body);
        self.review = Some(review);
        self.selected = 0;
        self.top = 0;
        self.reveal = true;
        if !context_changed && self.review.as_ref().unwrap().conflicts.is_empty() {
            self.accept_draft();
        } else {
            self.message = None;
        }
    }
    pub fn accept_draft(&mut self) {
        let Some(mut review) = self.review.take() else {
            return;
        };
        for conflict in &review.conflicts {
            if conflict.mine == Some(true) {
                review
                    .values
                    .insert(conflict.id.clone(), conflict.draft.clone());
            }
        }
        let cursors: BTreeMap<_, _> = self
            .editors
            .iter()
            .filter(|(id, editor)| {
                review.values.get(*id).and_then(Value::as_str) == Some(editor.text())
            })
            .map(|(id, editor)| (id.clone(), editor.cursor()))
            .collect();
        self.install(review.page);
        self.view = Some(review.view);
        self.drafts = review.values;
        for (id, editor) in &mut self.editors {
            let initial = editor.text().to_owned();
            editor.clear_if_unchanged(&initial);
            editor.insert(self.drafts[id].as_str().expect("validated text field"));
            editor.clear_history();
            if let Some(cursor) = cursors.get(id) {
                editor
                    .restore_cursor(*cursor)
                    .expect("unchanged text cursor");
            }
        }
        self.message = Some(Message::Local("extensions-draft-ready"));
    }
    pub fn contextual_actions(&self) -> Vec<Command> {
        if self.review.is_some() {
            return vec![Command::CancelDraft, Command::ApplyDraft];
        }
        if self.confirm_discard {
            return vec![Command::CancelDiscard, Command::ConfirmDiscard];
        }
        if self
            .unresolved
            .as_ref()
            .is_some_and(|pending| pending.recovery.is_some())
        {
            let mut commands = vec![Command::Reconcile];
            if self.unrecorded {
                commands.push(Command::Retry);
            }
            return commands;
        }
        if self.blocked && self.unresolved.is_none() && self.page.is_some() {
            return vec![Command::ResumeDraft];
        }
        vec![]
    }
}

pub(super) fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let colors = app.theme.colors();
    let state = &app.extensions;
    let review = state.review.as_ref().unwrap();
    let locale = app.i18n.locale().id();
    let width = area.width.saturating_sub(2);
    if width == 0 || area.height == 0 {
        return;
    }
    let mut lines = vec![
        Line::styled(
            app.i18n.text("extensions-draft-review"),
            Style::default().fg(colors.accent),
        ),
        Line::default(),
    ];
    let mut controls = Vec::new();
    let mut text = |value: &str, style: Style| {
        lines.extend(
            crate::pages::chat::layout::plain(value, width)
                .unwrap()
                .lines
                .into_iter()
                .map(|line| line.line.style(style)),
        );
    };
    let original = state.page.as_ref().unwrap();
    if original.title != review.page.title || original.body != review.page.body {
        for (key, page) in [
            ("extensions-previous-form", original),
            ("extensions-current-form", &review.page),
        ] {
            text(&app.i18n.text(key), Style::default().fg(colors.muted));
            text(
                &format!("{}\n{}\n", page.title.resolve(locale), page.body),
                Style::default().fg(colors.foreground),
            );
        }
    }
    for (index, conflict) in review.conflicts.iter().enumerate() {
        let field = review
            .page
            .fields
            .iter()
            .find(|field| field.id == conflict.id)
            .unwrap();
        lines.push(Line::styled(
            field.label.resolve(locale).to_owned(),
            Style::default().fg(colors.accent),
        ));
        lines.push(Line::default());
        for mine in [true, false] {
            let command = Command::DraftChoice(index, mine);
            let mark = if conflict.mine == Some(mine) {
                app.chrome.symbol("●", "[x]")
            } else {
                app.chrome.symbol("○", "[ ]")
            };
            let row = lines.len();
            lines.push(Line::raw(format!(
                "{mark} {}",
                app.i18n.text(command.label())
            )));
            let current = value(&field.control);
            let content = if mine { &conflict.draft } else { &current };
            let content = content
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| content.to_string());
            lines.extend(
                crate::pages::chat::layout::plain(&content, width)
                    .unwrap()
                    .lines
                    .into_iter()
                    .map(|line| line.line),
            );
            controls.push((row, lines.len() - row, command));
            lines.push(Line::default());
        }
    }
    let total = lines.len();
    let state = &mut app.extensions;
    if state.reveal
        && let Some((row, height, _)) = controls.get(state.selected)
        && (*row < state.top
            || *row + (*height).min(usize::from(area.height))
                > state.top + usize::from(area.height))
    {
        state.top = *row;
    }
    state.reveal = false;
    state.top = state
        .top
        .min(total.saturating_sub(usize::from(area.height)));
    state.area = Some(area);
    let top = state.top;
    for (index, (row, height, command)) in controls.iter().enumerate() {
        if app.focus == Focus::List && state.selected == index
            || app.hover.as_ref() == Some(&Action::Extension(command.clone()))
        {
            for line in lines.iter_mut().skip(*row).take(*height) {
                *line = line.clone().style(Style::default().fg(colors.accent));
            }
        }
        let end = (row + height).min(top + usize::from(area.height));
        let start = (*row).max(top);
        if start < end {
            app.hits.push(crate::app::Hit {
                area: Rect::new(
                    area.x,
                    area.y + (start - top) as u16,
                    width,
                    (end - start) as u16,
                ),
                action: Action::Extension(command.clone()),
            });
        }
    }
    frame.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(top)
                .take(usize::from(area.height))
                .collect::<Vec<_>>(),
        ),
        area,
    );
    if total > usize::from(area.height) {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(colors.subtle)),
            area,
            &mut ScrollbarState::new(total)
                .position(top)
                .viewport_content_length(usize::from(area.height)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{
        Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    fn draft() -> App {
        let mut app = super::super::tests::app();
        let editor = app.extensions.editors.get_mut("name").unwrap();
        editor.clear_if_unchanged("My notes");
        editor.insert("本地草稿🦀");
        app.extensions
            .drafts
            .insert("name".into(), json!(editor.text()));
        app.extensions.disconnect();
        app
    }
    fn reload(app: &mut App, page: Page) {
        app.extensions_action(Command::ResumeDraft);
        let request = app.extensions_request().unwrap();
        assert!(!request.needs_checkpoint());
        assert!(matches!(
            request.work,
            Work::Rebind {
                input: Input::Read { .. },
                ..
            }
        ));
        let mut view = app.extensions.view.clone().unwrap();
        view.target.registration = uuid::Uuid::new_v4();
        app.extensions_complete(
            request,
            Ok(Output::Rebound {
                view: Box::new(view),
                reply: Reply::Page { page },
            }),
        );
    }
    fn draw(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::view::draw(frame, app))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
    #[test]
    fn draft_reload_merges_disjoint_edits_and_keeps_new_revision_without_saving() {
        let mut app = draft();
        let saved = app.extensions.checkpoint("root").unwrap();
        app.extensions = State::default();
        app.extensions.restore(saved).unwrap();
        assert!(app.extensions_request().is_none());
        let cursor = app.extensions.editors["name"].cursor();
        let mut current = app.extensions.page.clone().unwrap();
        current.revision = "two".into();
        current.fields[0].control = Control::Toggle { value: false };
        reload(&mut app, current);
        assert!(app.extensions.review.is_none());
        assert!(app.extensions_enabled(&Command::Submit(0)));
        assert_eq!(app.extensions.drafts["enabled"], json!(false));
        assert_eq!(app.extensions.drafts["name"], json!("本地草稿🦀"));
        assert_eq!(app.extensions.editors["name"].cursor(), cursor);
        assert!(app.extensions_request().is_none(), "resume never submits");
        app.extensions
            .checkpoint("root")
            .unwrap()
            .validate("root")
            .unwrap();
        app.extensions_action(Command::Submit(0));
        let request = app.extensions_request().unwrap();
        assert!(
            matches!(&request.work, Work::Page { input: Input::Submit { revision, fields, .. }, .. } if revision == "two" && fields["name"] == json!("本地草稿🦀") && fields["enabled"] == json!(false))
        );
        assert!(!app.extensions_enabled(&Command::ResumeDraft));
    }

    #[test]
    fn conflicting_fields_need_explicit_choice_and_cancel_or_disconnect_keeps_original_draft() {
        let mut app = draft();
        let mut current = app.extensions.page.clone().unwrap();
        current.revision = "two".into();
        current.fields[1].control = Control::Text {
            value: "Changed remotely".into(),
            max_bytes: 128,
            multiline: false,
        };
        reload(&mut app, current.clone());
        assert!(!app.extensions_enabled(&Command::ApplyDraft));
        let screen = draw(&mut app, 58, 24);
        assert!(
            screen.replace(' ', "").contains("本地草稿🦀") && screen.contains("Changed remotely"),
            "{screen}"
        );
        let selected = app
            .hits
            .iter()
            .find(|hit| hit.action == Action::Extension(Command::DraftChoice(0, false)))
            .unwrap()
            .area;
        // The entire content row is clickable, not only the choice marker.
        app.input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: selected.x + 3,
            row: selected.y + 1,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(app.extensions_enabled(&Command::ApplyDraft));
        app.input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(app.extensions.review.is_none());
        assert_eq!(app.extensions.drafts["name"], json!("本地草稿🦀"));
        reload(&mut app, current.clone());
        draw(&mut app, 58, 24);
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        app.extensions_action(Command::ApplyDraft);
        assert_eq!(app.extensions.drafts["name"], json!("本地草稿🦀"));
        assert_eq!(app.extensions.page.as_ref().unwrap().revision, "two");
        assert!(app.extensions_request().is_none());

        app.extensions.disconnect();
        current.body = "Changed context".into();
        reload(&mut app, current);
        assert!(app.extensions.review.is_some());
        let screen = draw(&mut app, 58, 24);
        assert!(screen.contains("Previous form") && screen.contains("Current form"));
        draw(&mut app, 25, 8);
        assert!(app.extensions.area.is_none());
        app.extensions.disconnect();
        assert!(app.extensions.review.is_none());
        assert!(app.extensions.blocked);
        assert_eq!(app.extensions.drafts["name"], json!("本地草稿🦀"));
    }

    #[test]
    fn field_removal_disable_type_and_capacity_changes_never_truncate_a_dirty_value() {
        for change in 0..4 {
            let mut app = draft();
            let before = app.extensions.drafts.clone();
            let mut current = app.extensions.page.clone().unwrap();
            current.revision = "two".into();
            match change {
                0 => {
                    current.fields.remove(1);
                    current.actions[0].fields.pop();
                }
                1 => current.fields[1].enabled = false,
                2 => current.fields[1].control = Control::Toggle { value: false },
                _ => {
                    current.fields[1].control = Control::Text {
                        value: String::new(),
                        max_bytes: 1,
                        multiline: false,
                    }
                }
            }
            reload(&mut app, current);
            assert!(app.extensions.review.is_none());
            assert!(app.extensions.blocked);
            assert_eq!(app.extensions.drafts, before);
            assert_eq!(app.extensions.page.as_ref().unwrap().revision, "one");
            assert!(app.extensions_request().is_none());
        }
    }
}
