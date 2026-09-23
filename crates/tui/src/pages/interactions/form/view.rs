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
use crate::{i18n::I18n, pages::chat::layout};
use maka_protocol::capability::form::FormFormat;
use ratatui::{Frame, layout::Rect, style::Style, text::Line, widgets::Paragraph};

type Row = (Line<'static>, Option<Command>);
impl Form {
    fn lines(
        &self,
        width: u16,
        i18n: &I18n,
        ascii: bool,
        colors: crate::theme::Palette,
    ) -> Vec<Row> {
        let mut rows = vec![];
        let mut append = |text: String, command: Option<Command>| {
            let style = if command.is_some_and(|command| command == self.focus) {
                colors.selected()
            } else if command.is_some() {
                Style::default().fg(colors.accent)
            } else {
                Style::default()
            };
            if let Ok(layout) = layout::plain(&text, width) {
                rows.extend(
                    layout
                        .lines
                        .into_iter()
                        .map(|line| (line.line.style(style), command)),
                );
            }
        };
        append(self.message.clone(), None);
        append(
            i18n.format(
                "form-requester",
                &[
                    ("name", &self.requester.name),
                    ("source", self.requester.source.as_deref().unwrap_or("")),
                ],
            ),
            None,
        );
        let Some(field) = self.fields.get(self.current) else {
            return rows;
        };
        append(
            format!(
                "{} · {} {}",
                i18n.format(
                    "form-position",
                    &[
                        ("index", &(self.current + 1).to_string()),
                        ("count", &self.fields.len().to_string())
                    ]
                ),
                field.label,
                i18n.text(if field.required {
                    "form-required-label"
                } else {
                    "form-optional-label"
                })
            ),
            None,
        );
        if let Some(description) = &field.description {
            append(description.clone(), None);
        }
        let constraints = match &field.spec {
            FormFieldSpec::String {
                min_length,
                max_length,
                format,
                ..
            } => {
                let kind = i18n.text(match format {
                    None => "form-string",
                    Some(FormFormat::Email) => "form-email",
                    Some(FormFormat::Uri) => "form-uri",
                    Some(FormFormat::Date) => "form-date",
                    Some(FormFormat::DateTime) => "form-date-time",
                });
                format!(
                    "{kind} · {}",
                    i18n.format(
                        "form-length",
                        &[
                            ("min", &min_length.unwrap_or(0).to_string()),
                            ("max", &max_length.unwrap_or(2048).to_string())
                        ]
                    )
                )
            }
            FormFieldSpec::Number {
                minimum, maximum, ..
            }
            | FormFieldSpec::Integer {
                minimum, maximum, ..
            } => {
                let kind = i18n.text(if matches!(field.spec, FormFieldSpec::Integer { .. }) {
                    "form-integer"
                } else {
                    "form-number"
                });
                format!(
                    "{kind} · {}",
                    i18n.format(
                        "form-range",
                        &[
                            (
                                "min",
                                &minimum
                                    .map(|n| n.to_string())
                                    .unwrap_or_else(|| i18n.text("form-unbounded"))
                            ),
                            (
                                "max",
                                &maximum
                                    .map(|n| n.to_string())
                                    .unwrap_or_else(|| i18n.text("form-unbounded"))
                            )
                        ]
                    )
                )
            }
            FormFieldSpec::Boolean { .. } => i18n.text("form-boolean"),
            FormFieldSpec::SingleSelect { .. } => i18n.text("form-single"),
            FormFieldSpec::MultiSelect {
                min_items,
                max_items,
                options,
                ..
            } => i18n.format(
                "form-items",
                &[
                    ("min", &min_items.unwrap_or(0).to_string()),
                    ("max", &max_items.unwrap_or(options.len()).to_string()),
                ],
            ),
        };
        append(constraints, None);
        let draft = &self.drafts[self.current];
        for index in 0..self.options() {
            let (label, chosen) = match &field.spec {
                FormFieldSpec::Boolean { .. } => (
                    i18n.text(if index == 0 {
                        "form-true"
                    } else {
                        "form-false"
                    }),
                    draft.value == Some(FormValue::Boolean(index == 0)),
                ),
                FormFieldSpec::SingleSelect { options, .. } => (
                    options[index].label.clone(),
                    draft.value == Some(FormValue::String(options[index].value.clone())),
                ),
                FormFieldSpec::MultiSelect { options, .. } => (
                    options[index].label.clone(),
                    matches!(&draft.value,Some(FormValue::Strings(values)) if values.contains(&options[index].value)),
                ),
                _ => unreachable!(),
            };
            let multi = matches!(field.spec, FormFieldSpec::MultiSelect { .. });
            let mark = match (chosen && draft.present, multi, ascii) {
                (true, true, _) => "[x]",
                (false, true, _) => "[ ]",
                (true, false, true) => "(*)",
                (false, false, true) => "( )",
                (true, false, false) => "●",
                (false, false, false) => "○",
            };
            append(format!("{mark} {label}"), Some(Command::Option(index)));
        }
        for command in [Command::Empty, Command::Omit] {
            if self.accepts(command) {
                let chosen = command == Command::Omit && !draft.present;
                append(
                    format!(
                        "{} {}",
                        if chosen { "[x]" } else { "[ ]" },
                        i18n.text(command.label())
                    ),
                    Some(command),
                );
            }
        }
        rows
    }
    fn entry_height(&mut self, width: u16, maximum: u16) -> u16 {
        if self.editable() {
            self.drafts[self.current]
                .editor
                .preferred_height(width, maximum.max(1))
        } else {
            0
        }
    }
    pub fn preferred_height(
        &mut self,
        width: u16,
        i18n: &I18n,
        ascii: bool,
        button_rows: u16,
    ) -> u16 {
        let rows = self
            .lines(width, i18n, ascii, crate::theme::Palette::default())
            .len();
        // Borders, navigation, status/help and the actual wrapped footer.
        rows.saturating_add(5 + usize::from(button_rows) + usize::from(self.entry_height(width, 3)))
            .min(usize::from(u16::MAX)) as u16
    }
    pub fn draw(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        i18n: &I18n,
        ascii: bool,
        colors: crate::theme::Palette,
    ) -> Vec<Hit> {
        let mut hits = vec![];
        if area.is_empty() {
            self.invalidate_geometry();
            return hits;
        }
        let mut x = area.x;
        for (command, key) in [
            (
                self.current.checked_sub(1).map(Command::Field),
                "form-previous",
            ),
            (
                (self.current + 1 < self.fields.len()).then_some(Command::Field(self.current + 1)),
                "form-next",
            ),
        ] {
            let Some(command) = command else {
                continue;
            };
            let label = format!(" {} ", i18n.text(key));
            let width = unicode_width::UnicodeWidthStr::width(label.as_str())
                .min(usize::from(area.right().saturating_sub(x))) as u16;
            let rect = Rect::new(x, area.y, width, 1);
            frame.render_widget(
                Paragraph::new(label).style(if self.focus == command {
                    colors.selected()
                } else {
                    Style::default().fg(colors.accent)
                }),
                rect,
            );
            if !rect.is_empty() {
                hits.push(Hit {
                    area: rect,
                    action: Action::Interaction(command),
                });
            }
            x = x.saturating_add(width);
        }
        let entry_height = self
            .entry_height(area.width, area.height.saturating_sub(2).min(3))
            .min(area.height.saturating_sub(1));
        let body = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1 + entry_height),
        );
        let rows = self.lines(body.width, i18n, ascii, colors);
        self.max_scroll = rows.len().saturating_sub(usize::from(body.height));
        if self.reveal {
            if let Some(index) = rows
                .iter()
                .position(|(_, command)| *command == Some(self.focus))
            {
                if index < self.scroll {
                    self.scroll = index;
                } else if index >= self.scroll + usize::from(body.height) {
                    self.scroll = (index + 1).saturating_sub(usize::from(body.height));
                }
            }
            self.reveal = false;
        }
        self.scroll = self.scroll.min(self.max_scroll);
        for (index, (line, command)) in rows
            .into_iter()
            .skip(self.scroll)
            .take(usize::from(body.height))
            .enumerate()
        {
            let rect = Rect::new(body.x, body.y + index as u16, body.width, 1);
            frame.render_widget(Paragraph::new(line), rect);
            if let Some(command) = command {
                hits.push(Hit {
                    area: rect,
                    action: Action::Interaction(command),
                });
            }
        }
        if entry_height > 0 {
            let entry = Rect::new(
                area.x,
                area.bottom() - entry_height,
                area.width,
                entry_height,
            );
            let draft = &mut self.drafts[self.current];
            draft
                .editor
                .draw(frame, entry, self.focus == Command::FreeText, colors);
            if draft.editor.text().is_empty() {
                frame.render_widget(
                    Paragraph::new(i18n.text(if draft.present {
                        "form-empty-value"
                    } else {
                        "form-input"
                    }))
                    .style(Style::default().fg(colors.subtle)),
                    entry,
                );
            }
            hits.push(Hit {
                area: entry,
                action: Action::Interaction(Command::FreeText),
            });
        }
        hits
    }
}
