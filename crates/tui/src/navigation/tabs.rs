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
/// The sidebar lists every session; tabs are the open subset that keeps
/// drafts and Ctrl+PgUp/PgDn order.
#[derive(Default)]
pub struct Tabs {
    pub entries: Vec<Tab>,
}

pub struct Tab {
    pub id: String,
    pub name: Option<String>,
}

pub const LIMIT: usize = 32;

impl Tabs {
    pub fn contains(&self, id: &str) -> bool {
        self.entries.iter().any(|tab| tab.id == id)
    }

    pub fn open(&mut self, id: &str) -> bool {
        if self.contains(id) {
            return true;
        }
        if self.entries.len() == LIMIT {
            return false;
        }
        self.entries.push(Tab {
            id: id.into(),
            name: None,
        });
        true
    }

    pub fn close(&mut self, id: &str) -> Option<String> {
        let index = self.entries.iter().position(|tab| tab.id == id)?;
        self.entries.remove(index);
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
