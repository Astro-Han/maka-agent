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

use alacritty_terminal::{
    Term,
    grid::Dimensions,
    index::{Column, Line},
    term::{Config, TermMode, cell::Flags},
    vte::ansi,
};
use maka_runtime::terminal::{
    MouseEncoding, MouseTracking, TerminalCursor, TerminalInputModes, TerminalScreen, TerminalSize,
};
use std::{cell::RefCell, rc::Rc};

mod handler;
use handler::{Events, State};

const HISTORY: usize = 500;
const MAX_WRITE: usize = 64 * 1024;
const MAX_TEXT: usize = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("terminal screen failed: {0}")]
pub struct ScreenError(&'static str);

/// Pure terminal state. No OS handles, runtime, threads, or external effects.
/// Parsing is synchronous; callers own scheduling, backpressure and persistence.
pub struct Screen {
    parser: ansi::Processor,
    state: State,
    strings: StringLimit,
    failed: bool,
}

impl Screen {
    pub fn new(size: TerminalSize) -> Self {
        let events = Events(Rc::new(RefCell::new(Default::default())));
        let term = Term::new(
            Config {
                scrolling_history: HISTORY,
                ..Default::default()
            },
            &Size(size),
            events.clone(),
        );
        Self {
            parser: ansi::Processor::new(),
            state: State::new(term, events),
            strings: StringLimit::default(),
            failed: false,
        }
    }

    pub fn write(&mut self, mut text: &str) -> Result<String, ScreenError> {
        self.check()?;
        if text.len() > MAX_WRITE {
            return Err(ScreenError("write exceeds 64 KiB"));
        }
        self.state.begin();
        // Native transport decoding belongs to the caller. Keep parser cuts
        // on UTF-8 boundaries too: vte's partial-codepoint path can consume a
        // following ASCII character when its four-byte lookahead spans both.
        while !text.is_empty() {
            let end = text.floor_char_boundary(256.min(text.len()));
            let chunk = &text.as_bytes()[..end];
            text = &text[end..];
            if self.strings.feed(chunk).is_err() {
                self.failed = true;
                return Err(ScreenError("terminal string exceeds 64 KiB"));
            }
            self.parser.advance(&mut self.state, chunk);
            // We have no renderer to batch. Flush synchronized output at the
            // parser cut so replies and persisted state never wait for repaint.
            self.parser.stop_sync(&mut self.state);
            if self.state.failed() {
                self.failed = true;
                return Err(ScreenError("terminal expansion or reply budget exceeded"));
            }
        }
        Ok(self.state.take_replies())
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<(), ScreenError> {
        self.check()?;
        let previous = self.size();
        self.state.term.resize(Size(size));
        self.state.resized();
        self.state.truncated |= self.state.term.history_size() == HISTORY
            && (size.cols() < previous.cols() || size.rows() < previous.rows());
        Ok(())
    }

    pub fn reset_after_gap(&mut self) {
        let size = self.size();
        *self = Self::new(size);
        self.state.truncated = true;
    }

    pub fn size(&self) -> TerminalSize {
        TerminalSize::new(
            self.state.term.columns() as u16,
            self.state.term.screen_lines() as u16,
        )
        .expect("validated terminal dimensions")
    }

    pub fn snapshot(&self) -> Result<TerminalScreen, ScreenError> {
        self.check()?;
        let term = &self.state.term;
        let mode = term.mode();
        let (screen, scrollback, clipped) = render(term, true);
        let point = term.grid().cursor.point;
        Ok(TerminalScreen {
            screen,
            scrollback,
            last_alternate_screen: self.state.last_alternate.clone(),
            size: self.size(),
            cursor: TerminalCursor {
                x: point.column.0 as u16,
                y: point.line.0.max(0) as u16,
                visible: mode.contains(TermMode::SHOW_CURSOR),
            },
            alternate_screen: mode.contains(TermMode::ALT_SCREEN),
            truncated: self.state.truncated || clipped,
            input: TerminalInputModes {
                application_cursor_keys_mode: mode.contains(TermMode::APP_CURSOR),
                mouse_tracking_mode: self.state.mouse,
                mouse_encoding: self.state.encoding,
            },
        })
    }

    fn check(&self) -> Result<(), ScreenError> {
        if self.failed {
            Err(ScreenError("parser is closed after a failed operation"))
        } else {
            Ok(())
        }
    }
}

struct Size(TerminalSize);
impl Dimensions for Size {
    fn columns(&self) -> usize {
        self.0.cols().into()
    }
    fn screen_lines(&self) -> usize {
        self.0.rows().into()
    }
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
}

/// Bound upstream's OSC accumulator across arbitrary native read boundaries.
/// Other escape strings have no unbounded storage in vte.
#[derive(Default)]
struct StringLimit {
    escaped: bool,
    osc: Option<usize>,
}
impl StringLimit {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), ()> {
        for &byte in bytes {
            if let Some(length) = &mut self.osc {
                if matches!(byte, 7 | 0x18 | 0x1a | 0x1b) {
                    self.osc = None;
                    self.escaped = byte == 0x1b;
                } else {
                    *length += 1;
                    if *length > MAX_WRITE {
                        return Err(());
                    }
                }
            } else if byte == 0x1b {
                self.escaped = true;
            } else if self.escaped {
                match byte {
                    b']' => {
                        self.osc = Some(0);
                        self.escaped = false;
                    }
                    0..=0x17 | 0x19 | 0x1c..=0x1f => {}
                    _ => self.escaped = false,
                }
            }
        }
        Ok(())
    }
}

fn render(term: &Term<Events>, history: bool) -> (String, String, bool) {
    let grid = term.grid();
    let mut screen = String::new();
    let mut clipped = false;
    let mut remaining = MAX_TEXT;
    // Preserve the current viewport before spending the snapshot budget on
    // history. Keep the newest history when old output must be dropped.
    for y in 0..term.screen_lines() as i32 {
        let (text, overflow) = render_row(term, y, remaining.saturating_sub(1));
        if y != 0 {
            screen.push('\n');
        }
        screen.push_str(&text);
        remaining = MAX_TEXT.saturating_sub(screen.len());
        clipped |= overflow;
        if overflow {
            break;
        }
    }
    while screen.ends_with('\n') {
        screen.pop();
    }
    remaining = MAX_TEXT - screen.len();
    let mut lines = Vec::new();
    if history {
        for y in (-(term.history_size() as i32)..0).rev() {
            let (text, overflow) = render_row(term, y, remaining.saturating_sub(1));
            if overflow || remaining == 0 {
                clipped = true;
                break;
            }
            remaining = remaining.saturating_sub(text.len() + 1);
            let wrapped = grid[Line(y)][Column(term.columns() - 1)]
                .flags
                .contains(Flags::WRAPLINE);
            lines.push((text, wrapped));
        }
    }
    let mut scrollback = String::new();
    let mut wrapped = true;
    for (text, continues) in lines.into_iter().rev() {
        if !wrapped {
            scrollback.push('\n');
        }
        scrollback.push_str(&text);
        wrapped = continues;
    }
    (screen, scrollback, clipped)
}

fn render_row(term: &Term<Events>, y: i32, budget: usize) -> (String, bool) {
    let row = &term.grid()[Line(y)];
    let mut text = String::new();
    for x in 0..term.columns() {
        let cell = &row[Column(x)];
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        for c in std::iter::once(cell.c).chain(cell.zerowidth().unwrap_or_default().iter().copied())
        {
            if text.len() + c.len_utf8() > budget {
                return (text, true);
            }
            text.push(c);
        }
    }
    text.truncate(text.trim_end_matches(' ').len());
    (text, false)
}
