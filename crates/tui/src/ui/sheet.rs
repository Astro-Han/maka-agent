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

//! Sheets, the one modal dialog shape: a bold title inside the box, prose,
//! a body, and buttons at the bottom right. The chosen button holds focus
//! when a sheet opens, Tab cycles inside it, and Esc or a click outside asks
//! its owner to dismiss it. Nothing beneath a sheet sees input.
use super::{
    layout,
    node::{Node, On, Role, Size, Tone},
    surface::{Context, Outcome, Surface},
};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Margin, Position, Rect},
    style::{Modifier, Style},
    widgets::{Block, BorderType},
};

const ROOT: &str = "sheet";
const WIDTH: u16 = 64;
/// Narrower than this, prose wraps into a column nobody can read.
const MIN_WIDTH: u16 = 28;

pub struct Sheet<M> {
    key: String,
    title: String,
    body: Vec<Node<M>>,
    buttons: Vec<Node<M>>,
    focus: Option<&'static str>,
}

impl<M> Sheet<M> {
    /// `key` names what the sheet shows; a different key opens fresh, with
    /// the chosen button focused again.
    pub fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            body: vec![],
            buttons: vec![],
            focus: None,
        }
    }

    /// Prose wrapped per source line; blank lines stay as spacing.
    pub fn text(self, key: &'static str, text: &str, tone: Tone) -> Self {
        let lines = text
            .lines()
            .enumerate()
            .map(|(index, line)| Node::text(index.to_string(), vec![(line.to_owned(), tone)]))
            .collect();
        self.body(Node::column(key, lines))
    }

    pub fn body(mut self, node: Node<M>) -> Self {
        self.body.push(node);
        self
    }

    pub fn button(
        mut self,
        key: &'static str,
        label: String,
        role: Role,
        message: M,
        enabled: bool,
    ) -> Self {
        self.buttons.push(
            Node::button(key, label, role)
                .on(On::Activate(message))
                .enabled(enabled),
        );
        self
    }

    /// The button focused on open. Confirmations of a change choose Cancel,
    /// so Enter alone never commits it.
    pub fn focus(mut self, key: &'static str) -> Self {
        self.focus = Some(key);
        self
    }

    fn footer_width(&self) -> u16 {
        let gaps = 2 * self.buttons.len().saturating_sub(1) as u16;
        self.buttons.iter().map(layout::width).sum::<u16>() + gaps
    }

    fn tree(self) -> Node<M> {
        let mut children = vec![Node::text("title", vec![(self.title, Tone::Strong)])];
        children.extend(self.body);
        let mut footer = vec![Node::text("space", vec![]).size(Size::Fill)];
        footer.extend(self.buttons);
        children.push(Node::row("footer", footer).gap(2));
        Node::column(ROOT, children).gap(1)
    }
}

/// The modal layer that presents one sheet at a time.
pub struct Layer<M> {
    surface: Surface<M>,
    shown: Option<String>,
    /// The drawn box; clicks outside it dismiss.
    area: Option<Rect>,
}

impl<M> Default for Layer<M> {
    fn default() -> Self {
        Self {
            surface: Surface::default(),
            shown: None,
            area: None,
        }
    }
}

impl<M: Clone> Layer<M> {
    /// Centers the sheet over `area`. Returns false, leaving nothing
    /// clickable, when it does not fit; the caller says so instead.
    pub fn render(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        sheet: Sheet<M>,
        context: Context,
    ) -> bool {
        let width = WIDTH.min(area.width.saturating_sub(2));
        // Border and padding: two cells a side, one row above and below.
        let inner = width.saturating_sub(4);
        let fits_footer = inner >= sheet.footer_width().max(MIN_WIDTH);
        let key = sheet.key.clone();
        let focus = sheet.focus.map(|button| format!("{ROOT}/footer/{button}"));
        let tree = sheet.tree();
        let height = layout::height(&tree, inner).saturating_add(4);
        if !fits_footer || height > area.height.saturating_sub(2) {
            self.area = None;
            self.surface.invalidate();
            return false;
        }
        if self.shown.as_ref() != Some(&key) {
            self.surface = Surface::default();
            if let Some(focus) = focus {
                self.surface.focus(focus);
            }
            self.shown = Some(key);
        }
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        // The page stays visible but recedes, so the sheet is what reads.
        frame
            .buffer_mut()
            .set_style(area, Style::default().add_modifier(Modifier::DIM));
        crate::view::clear_overlay(frame, rect);
        let block = Block::bordered()
            .border_type(if context.ascii {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(Style::default().fg(context.colors.border))
            .style(context.colors.base());
        // One cell of breathing room inside the border on every side.
        let content = block.inner(rect).inner(Margin::new(1, 1));
        frame.render_widget(block, rect);
        let context = Context {
            focused: true,
            ..context
        };
        self.surface.render(frame, content, tree, context);
        self.area = Some(rect);
        true
    }

    /// Geometry changed: nothing is clickable until the next draw.
    pub fn invalidate(&mut self) {
        self.area = None;
        self.surface.invalidate();
    }

    /// No sheet is shown; the next one opens fresh.
    pub fn close(&mut self) {
        self.shown = None;
        self.area = None;
        self.surface.leave();
    }

    /// Modal: every key, paste and pointer event is consumed except shell
    /// chords (Ctrl, Alt, Super), which stay reachable for quitting. `dismiss`
    /// is what Esc and a click outside the box send.
    pub fn input(&mut self, event: &Event, dismiss: M) -> Outcome<M> {
        let consumed = |outcome: Outcome<M>| Outcome {
            consumed: true,
            ..outcome
        };
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.modifiers.intersects(
                    KeyModifiers::CONTROL
                        | KeyModifiers::ALT
                        | KeyModifiers::SUPER
                        | KeyModifiers::META,
                ) {
                    return Outcome::ignored();
                }
                if key.code == KeyCode::Esc && !self.surface.captures() {
                    return Outcome::emit(dismiss);
                }
                let outcome = self.surface.input(event);
                if !outcome.consumed
                    && matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
                    && self.area.is_some()
                {
                    // Past either end, focus wraps: there is nowhere else to go.
                    self.surface.enter(
                        key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT),
                    );
                    return Outcome::handled(true);
                }
                consumed(outcome)
            }
            Event::Mouse(mouse) => {
                let outside = self
                    .area
                    .is_some_and(|area| !area.contains(Position::new(mouse.column, mouse.row)));
                if outside
                    && mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && !self.surface.captures()
                {
                    return Outcome::emit(dismiss);
                }
                consumed(self.surface.input(event))
            }
            Event::Resize(..) | Event::FocusLost | Event::FocusGained => self.surface.input(event),
            _ => Outcome::handled(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, MouseEvent};
    use ratatui::{Terminal, backend::TestBackend};

    #[derive(Clone, Debug, PartialEq)]
    enum Message {
        Cancel,
        Archive,
        Dismiss,
    }

    fn sheet(key: &str, archive: bool) -> Sheet<Message> {
        Sheet::new(key, "Put this session away?")
            .text(
                "note",
                "History is kept.\n\nYou can restore it later.",
                Tone::Subtle,
            )
            .button(
                "cancel",
                "Cancel".into(),
                Role::Normal,
                Message::Cancel,
                true,
            )
            .button(
                "archive",
                "Archive".into(),
                Role::Primary,
                Message::Archive,
                archive,
            )
            .focus("cancel")
    }

    fn draw(
        layer: &mut Layer<Message>,
        sheet: Sheet<Message>,
        width: u16,
        height: u16,
    ) -> (bool, Terminal<TestBackend>) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut shown = false;
        terminal
            .draw(|frame| {
                let context = Context {
                    colors: crate::theme::Palette::default(),
                    ascii: false,
                    focused: false,
                };
                shown = layer.render(frame, frame.area(), sheet, context);
            })
            .unwrap();
        (shown, terminal)
    }

    fn locate(terminal: &Terminal<TestBackend>, text: &str) -> (u16, u16) {
        let buffer = terminal.backend().buffer();
        for y in 0..buffer.area.height {
            let line: String = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            if let Some(byte) = line.find(text) {
                return (
                    unicode_width::UnicodeWidthStr::width(&line[..byte]) as u16,
                    y,
                );
            }
        }
        panic!("{text:?} is not on screen");
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    fn click(x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn sheets_focus_their_default_wrap_tab_and_dismiss_from_outside() {
        let mut layer = Layer::default();
        let (shown, terminal) = draw(&mut layer, sheet("a", true), 80, 24);
        assert!(shown);
        let mut send = |event: Event| layer.input(&event, Message::Dismiss).message;
        assert_eq!(
            send(key(KeyCode::Enter)),
            Some(Message::Cancel),
            "Enter alone commits nothing"
        );
        send(key(KeyCode::Tab));
        assert_eq!(send(key(KeyCode::Enter)), Some(Message::Archive));
        send(key(KeyCode::Tab));
        assert_eq!(
            send(key(KeyCode::Char(' '))),
            Some(Message::Cancel),
            "Tab past the last button wraps to the first"
        );
        let (x, y) = locate(&terminal, "Put this");
        assert_eq!(send(click(x, y)), None, "prose inside the box is inert");
        let (x, y) = locate(&terminal, "Archive");
        assert_eq!(send(click(x, y)), Some(Message::Archive));
        assert_eq!(send(click(0, 0)), Some(Message::Dismiss));
        let quit = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        let outcome = layer.input(&quit, Message::Dismiss);
        assert!(!outcome.consumed, "shell chords stay reachable");
        let paste = layer.input(&Event::Paste("text".into()), Message::Dismiss);
        assert!(
            paste.consumed && paste.message.is_none(),
            "nothing reaches the page"
        );
    }

    #[test]
    fn a_sheet_that_does_not_fit_presents_nothing_to_activate() {
        let mut layer = Layer::default();
        draw(&mut layer, sheet("a", true), 80, 24);
        layer.input(&key(KeyCode::Tab), Message::Dismiss);
        let (shown, _) = draw(&mut layer, sheet("a", true), 30, 8);
        assert!(!shown);
        let outcome = layer.input(&key(KeyCode::Enter), Message::Dismiss);
        assert!(outcome.consumed && outcome.message.is_none());
        assert_eq!(
            layer.input(&key(KeyCode::Esc), Message::Dismiss).message,
            Some(Message::Dismiss),
            "Esc still leaves"
        );
        // A new sheet starts from its default; a disabled button is skipped.
        draw(&mut layer, sheet("b", false), 80, 24);
        layer.input(&key(KeyCode::Tab), Message::Dismiss);
        assert_eq!(
            layer.input(&key(KeyCode::Enter), Message::Dismiss).message,
            Some(Message::Cancel)
        );
    }
}
