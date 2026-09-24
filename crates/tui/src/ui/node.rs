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

use std::borrow::Cow;

/// Main-axis size of a child inside its parent Row or Column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Content,
    Fixed(u16),
    Fill,
}

/// Semantic text roles; the kernel maps them to the active palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Strong,
    Muted,
    Subtle,
    Accent,
    Warning,
    /// One of the palette's stable identity hues (session titles).
    Hue(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

pub struct Choice<M> {
    pub label: String,
    pub action: M,
}

pub enum On<M> {
    /// Click, Enter or Space emits the action.
    Activate(M),
    /// Opens a kernel-owned popover; picking a choice emits its action.
    /// `current` is None when the active value is not one of the choices.
    Choose {
        choices: Vec<Choice<M>>,
        current: Option<usize>,
    },
}

pub enum Kind<M> {
    Column {
        children: Vec<Node<M>>,
        gap: u16,
    },
    Row {
        children: Vec<Node<M>>,
        gap: u16,
    },
    /// `clip` keeps one row and ends an overflow with an ellipsis.
    Text {
        spans: Vec<(String, Tone)>,
        align: Align,
        clip: bool,
    },
    /// A one-cell divider across the parent's cross axis.
    Rule,
    /// Vertical scrolling for content taller than its rectangle.
    Scroll(Box<Node<M>>),
}

/// A keyed node. Keys identify interaction state (focus, hover, scroll) across
/// frames, so they must be stable and unique among siblings; positions,
/// translated labels and coordinates are never identities.
pub struct Node<M> {
    pub key: Cow<'static, str>,
    pub size: Size,
    pub kind: Kind<M>,
    pub on: Option<On<M>>,
    pub enabled: bool,
    /// The chosen item of a selection group; independent of keyboard focus.
    pub current: bool,
    /// Keyboard focus arriving here also activates it, for lists whose
    /// selection follows the focus.
    pub follow_focus: bool,
    pub hint: Option<String>,
}

impl<M> Node<M> {
    fn new(key: impl Into<Cow<'static, str>>, kind: Kind<M>) -> Self {
        Self {
            key: key.into(),
            size: Size::Content,
            kind,
            on: None,
            enabled: true,
            current: false,
            follow_focus: false,
            hint: None,
        }
    }
    pub fn column(key: impl Into<Cow<'static, str>>, children: Vec<Node<M>>) -> Self {
        Self::new(key, Kind::Column { children, gap: 0 })
    }
    pub fn row(key: impl Into<Cow<'static, str>>, children: Vec<Node<M>>) -> Self {
        Self::new(key, Kind::Row { children, gap: 0 })
    }
    pub fn text(key: impl Into<Cow<'static, str>>, spans: Vec<(String, Tone)>) -> Self {
        Self::new(
            key,
            Kind::Text {
                spans,
                align: Align::Start,
                clip: false,
            },
        )
    }
    pub fn rule(key: impl Into<Cow<'static, str>>) -> Self {
        Self::new(key, Kind::Rule).size(Size::Fixed(1))
    }
    pub fn scroll(key: impl Into<Cow<'static, str>>, child: Node<M>) -> Self {
        Self::new(key, Kind::Scroll(Box::new(child))).size(Size::Fill)
    }
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
    pub fn gap(mut self, rows: u16) -> Self {
        if let Kind::Column { gap, .. } | Kind::Row { gap, .. } = &mut self.kind {
            *gap = rows;
        }
        self
    }
    pub fn align(mut self, to: Align) -> Self {
        if let Kind::Text { align, .. } = &mut self.kind {
            *align = to;
        }
        self
    }
    /// One row, truncated with an ellipsis instead of wrapping.
    pub fn clip(mut self) -> Self {
        if let Kind::Text { clip, .. } = &mut self.kind {
            *clip = true;
        }
        self
    }
    pub fn on(mut self, on: On<M>) -> Self {
        self.on = Some(on);
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }
    pub fn follow_focus(mut self) -> Self {
        self.follow_focus = true;
        self
    }
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}
