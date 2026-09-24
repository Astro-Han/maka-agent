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

//! Text selection across many text elements, for surfaces GPUI gives none:
//! the transcript's messages, blocks and table cells. Every text element
//! painted in a frame registers itself in paint order, which is document
//! order; a drag resolves into spans that keep a copy of their text, so a
//! selection survives its rows scrolling out of the list.

use gpui_kit::{
    Bounds, IntoElement, ParentElement, Pixels, Point, SharedString, Styled, TextLayout, canvas,
    div,
};
use std::{cell::RefCell, ops::Range, rc::Rc};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub row: SharedString,
    pub ordinal: u32,
}

struct Entry {
    key: Key,
    text: SharedString,
    /// First text of a block; copy puts a blank line before it.
    block_start: bool,
    layout: TextLayout,
}

impl Entry {
    fn bounds(&self) -> Bounds<Pixels> {
        self.layout.bounds()
    }

    fn offset(&self, point: Point<Pixels>) -> usize {
        let offset = match self.layout.index_for_position(point) {
            Ok(offset) | Err(offset) => offset.min(self.text.len()),
        };
        floor_char(&self.text, offset)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub key: Key,
    pub range: Range<usize>,
    text: SharedString,
    block_start: bool,
}

#[derive(Default)]
pub struct Selection {
    registry: Rc<RefCell<Vec<Entry>>>,
    anchor: Option<(Key, usize)>,
    dragging: bool,
    spans: Vec<Span>,
}

impl Selection {
    /// Paint this before any selectable text of the frame.
    pub fn frame_start(&self) -> impl IntoElement {
        let registry = self.registry.clone();
        canvas(
            |_, _, _| {},
            move |_, _, _, _| registry.borrow_mut().clear(),
        )
        .absolute()
        .size_0()
    }

    pub fn range_for(&self, key: &Key) -> Option<Range<usize>> {
        self.spans
            .iter()
            .find(|span| &span.key == key)
            .map(|span| span.range.clone())
    }

    /// Wraps a text element so it takes part in selection. `element` is the
    /// `StyledText` whose layout this is, possibly wrapped for link clicks.
    pub fn text(
        &self,
        key: Key,
        text: SharedString,
        block_start: bool,
        layout: TextLayout,
        element: impl IntoElement,
    ) -> impl IntoElement {
        let registry = self.registry.clone();
        div()
            .relative()
            .min_w_0()
            .cursor_text()
            .child(element)
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, _, _| {
                        registry.borrow_mut().push(Entry {
                            key,
                            text,
                            block_start,
                            layout,
                        })
                    },
                )
                .absolute()
                .size_0(),
            )
    }

    /// Returns whether the selection changed.
    pub fn mouse_down(&mut self, position: Point<Pixels>, clicks: usize) -> bool {
        let registry = self.registry.borrow();
        let Some(entry) = registry
            .iter()
            .find(|entry| entry.bounds().contains(&position))
        else {
            drop(registry);
            self.anchor = None;
            self.dragging = false;
            return !std::mem::take(&mut self.spans).is_empty();
        };
        let offset = entry.offset(position);
        let range = match clicks {
            1 => offset..offset,
            2 => word(&entry.text, offset),
            _ => line(&entry.text, offset),
        };
        let spans = if range.is_empty() {
            Vec::new()
        } else {
            vec![Span {
                key: entry.key.clone(),
                range: range.clone(),
                text: entry.text.clone(),
                block_start: false,
            }]
        };
        self.anchor = Some((entry.key.clone(), range.start));
        drop(registry);
        self.dragging = true;
        let changed = self.spans != spans;
        self.spans = spans;
        changed
    }

    pub fn mouse_move(&mut self, position: Point<Pixels>) -> bool {
        if !self.dragging {
            return false;
        }
        let Some((anchor_key, anchor_offset)) = self.anchor.clone() else {
            return false;
        };
        let registry = self.registry.borrow();
        let Some(anchor) = registry.iter().position(|entry| entry.key == anchor_key) else {
            return false;
        };
        let head = registry
            .iter()
            .position(|entry| {
                let bounds = entry.bounds();
                position.y >= bounds.top()
                    && position.y < bounds.bottom()
                    && position.x >= bounds.left() - gpui_kit::px(24.)
            })
            .or_else(|| {
                registry
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, entry)| {
                        let bounds = entry.bounds();
                        let distance = if position.y < bounds.top() {
                            bounds.top() - position.y
                        } else {
                            position.y - bounds.bottom()
                        };
                        (f32::from(distance).max(0.) * 100.) as i64
                    })
                    .map(|(ix, _)| ix)
            });
        let Some(head) = head else {
            return false;
        };
        let head_offset = registry[head].offset(position);
        let (first, first_offset, last, last_offset) =
            if (head, head_offset) < (anchor, anchor_offset) {
                (head, head_offset, anchor, anchor_offset)
            } else {
                (anchor, anchor_offset, head, head_offset)
            };
        let mut spans = Vec::new();
        for (ix, entry) in registry.iter().enumerate().take(last + 1).skip(first) {
            let start = if ix == first { first_offset } else { 0 };
            let end = if ix == last {
                last_offset
            } else {
                entry.text.len()
            };
            let crossed = ix != first && ix != last;
            if start < end || (crossed && entry.text.is_empty()) {
                spans.push(Span {
                    key: entry.key.clone(),
                    range: floor_char(&entry.text, start)..floor_char(&entry.text, end),
                    text: entry.text.clone(),
                    block_start: entry.block_start && !spans.is_empty(),
                });
            }
        }
        drop(registry);
        if spans == self.spans {
            return false;
        }
        self.spans = spans;
        true
    }

    pub fn mouse_up(&mut self) {
        self.dragging = false;
    }

    /// The selected text as it reads on screen.
    pub fn copied(&self) -> Option<String> {
        let mut out = String::new();
        for (ix, span) in self.spans.iter().enumerate() {
            if ix > 0 {
                out.push_str(if span.block_start { "\n\n" } else { "\n" });
            }
            out.push_str(&span.text[span.range.clone()]);
        }
        (!out.is_empty()).then_some(out)
    }
}

fn floor_char(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn word(text: &str, offset: usize) -> Range<usize> {
    text.split_word_bound_indices()
        .map(|(start, word)| start..start + word.len())
        .find(|range| range.contains(&offset) || range.end == offset && range.start < offset)
        .filter(|range| !text[range.clone()].trim().is_empty())
        .unwrap_or(offset..offset)
}

fn line(text: &str, offset: usize) -> Range<usize> {
    let start = text[..offset].rfind('\n').map_or(0, |at| at + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |at| offset + at);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(row: &str, ordinal: u32, text: &str, range: Range<usize>, block_start: bool) -> Span {
        Span {
            key: Key {
                row: row.to_owned().into(),
                ordinal,
            },
            range,
            text: text.to_owned().into(),
            block_start,
        }
    }

    #[test]
    fn copies_rendered_text_with_block_breaks() {
        let selection = Selection {
            spans: vec![
                span("a", 0, "Hello world", 6..11, false),
                span("a", 1, "cell one", 0..8, true),
                span("a", 2, "cell two", 0..4, false),
            ],
            ..Default::default()
        };
        assert_eq!(selection.copied().unwrap(), "world\n\ncell one\ncell");
    }

    #[test]
    fn double_click_takes_a_word_and_triple_a_line() {
        assert_eq!(word("run cargo_test now", 6), 4..14);
        assert_eq!(word("a  b", 2), 2..2);
        assert_eq!(line("one\ntwo three\nfour", 6), 4..13);
    }
}
