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

/// Open reading destinations, independent of route history and Host execution.
#[derive(Default)]
pub struct Tabs {
    pub entries: Vec<Tab>,
    pub top: usize,
    pub reveal: Option<usize>,
    pub area: Option<ratatui::layout::Rect>,
    drag: Option<(u16, usize)>,
}

pub struct Tab {
    pub id: String,
    pub name: Option<String>,
}

pub const LIMIT: usize = 32;

impl Tabs {
    pub fn capacity(area: ratatui::layout::Rect) -> usize {
        usize::from(if area.width >= 12 {
            area.height.div_ceil(2)
        } else {
            area.height
        })
    }

    pub fn scrollbar(&self) -> Option<(ratatui::layout::Rect, ratatui::layout::Rect)> {
        use ratatui::layout::Rect;
        let area = self.area?;
        let capacity = Self::capacity(area);
        if area.width < 12 || capacity == 0 || self.entries.len() <= capacity {
            return None;
        }
        let height = (usize::from(area.height) * capacity / self.entries.len()).max(1) as u16;
        let travel = area.height - height;
        let offset = self.top.min(self.entries.len() - capacity) * usize::from(travel)
            / (self.entries.len() - capacity);
        Some((
            Rect::new(area.right() - 1, area.y, 1, area.height),
            Rect::new(area.right() - 1, area.y + offset as u16, 1, height),
        ))
    }

    pub fn invalidate_geometry(&mut self) {
        self.area = None;
        self.drag = None;
    }

    pub fn mouse(&mut self, event: crossterm::event::MouseEvent) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        let Some(area) = self.area else {
            return false;
        };
        let maximum = self.entries.len().saturating_sub(Self::capacity(area));
        let point = (event.column, event.row).into();
        match event.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if area.contains(point) => {
                self.top = if event.kind == MouseEventKind::ScrollDown {
                    (self.top + 1).min(maximum)
                } else {
                    self.top.saturating_sub(1)
                };
            }
            MouseEventKind::Down(MouseButton::Left)
                if self
                    .scrollbar()
                    .is_some_and(|(track, _)| track.contains(point)) =>
            {
                let (track, thumb) = self.scrollbar().unwrap();
                if !thumb.contains(point) {
                    let offset = event.row.saturating_sub(track.y + thumb.height / 2);
                    self.top = (usize::from(offset) * maximum
                        / usize::from(track.height - thumb.height).max(1))
                    .min(maximum);
                }
                self.drag = Some((event.row, self.top));
            }
            MouseEventKind::Drag(MouseButton::Left) if self.drag.is_some() => {
                let (row, top) = self.drag.unwrap();
                let Some((track, thumb)) = self.scrollbar() else {
                    self.drag = None;
                    return true;
                };
                let delta = (i64::from(event.row) - i64::from(row)) * maximum as i64
                    / i64::from((track.height - thumb.height).max(1));
                self.top = (top as i64 + delta).clamp(0, maximum as i64) as usize;
            }
            MouseEventKind::Up(MouseButton::Left) if self.drag.is_some() => {
                self.drag = None;
            }
            _ => return false,
        }
        self.reveal = None;
        true
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.iter().any(|tab| tab.id == id)
    }

    pub fn open(&mut self, id: &str) -> bool {
        if self.contains(id) {
            self.reveal = self.entries.iter().position(|tab| tab.id == id);
            return true;
        }
        if self.entries.len() == LIMIT {
            return false;
        }
        self.entries.push(Tab {
            id: id.into(),
            name: None,
        });
        self.reveal = Some(self.entries.len() - 1);
        true
    }

    pub fn close(&mut self, id: &str) -> Option<String> {
        let index = self.entries.iter().position(|tab| tab.id == id)?;
        self.entries.remove(index);
        self.top = self.top.min(self.entries.len().saturating_sub(1));
        self.entries
            .get(index.min(self.entries.len().saturating_sub(1)))
            .map(|tab| tab.id.clone())
    }

    pub fn relative(&self, current: Option<&str>, forward: bool) -> Option<String> {
        if self.entries.is_empty() {
            return None;
        }
        let index = current.and_then(|id| self.entries.iter().position(|tab| tab.id == id));
        let next = match (index, forward) {
            (Some(index), true) => (index + 1) % self.entries.len(),
            (Some(index), false) => (index + self.entries.len() - 1) % self.entries.len(),
            (None, true) => 0,
            (None, false) => self.entries.len() - 1,
        };
        Some(self.entries[next].id.clone())
    }

    pub fn rename(&mut self, id: &str, name: &str) {
        if let Some(tab) = self.entries.iter_mut().find(|tab| tab.id == id) {
            // The label is a bounded display hint, never an entity identity.
            use unicode_segmentation::UnicodeSegmentation;
            tab.name = Some(crate::view::safe(name).graphemes(true).take(128).collect());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navigation::{Navigation, Route};

    #[test]
    fn tabs_keep_stable_order_and_close_removes_only_that_destination_from_history() {
        let mut tabs = Tabs::default();
        let mut nav = Navigation::default();
        for id in ["a", "b", "a", "c"] {
            assert!(tabs.open(id));
            nav.visit(Route::Session(id.into()));
        }
        tabs.rename("b", "Same name");
        tabs.rename("c", "Same name");
        assert_eq!(tabs.entries.len(), 3);
        assert_eq!(tabs.relative(Some("c"), true).as_deref(), Some("a"));
        assert_eq!(tabs.relative(Some("a"), false).as_deref(), Some("c"));
        nav.back(); // a, with c still ahead
        nav.close_session("b", Route::Workspace);
        tabs.close("b");
        assert_eq!(nav.current(), Route::Session("a".into()));
        nav.forward();
        assert_eq!(nav.current(), Route::Session("c".into()));
        let fallback = tabs.close("c").map_or(Route::Workspace, Route::Session);
        nav.close_session("c", fallback);
        assert_eq!(nav.current(), Route::Session("a".into()));
        for _ in 0..10 {
            nav.back();
            assert_ne!(nav.current(), Route::Session("c".into()));
        }
        for _ in 0..10 {
            nav.forward();
            assert_ne!(nav.current(), Route::Session("b".into()));
        }
        for index in 0..LIMIT - 1 {
            assert!(tabs.open(&index.to_string()));
        }
        assert!(!tabs.open("overflow"));
        assert!(tabs.open("a"));
        assert_eq!(tabs.entries.len(), LIMIT);
    }
}
