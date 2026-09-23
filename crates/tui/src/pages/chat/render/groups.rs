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

//! Local folds over canonical tool cards, never synthetic execution records.
use super::*;
use tools::{Activity, State};

impl MessageKey {
    fn group(&self) -> Self {
        Self {
            part: Part::Activity,
            ..self.clone()
        }
    }
}

impl Transcript {
    fn activity_member(&self, key: &MessageKey) -> bool {
        let block = &self.blocks[key];
        block.activity.is_some()
            && matches!(block.kind, Kind::Tool(State::Pending | State::Returned))
    }

    pub(super) fn reconcile_groups(&mut self, i18n: &I18n) {
        // Diagnostic rows deliberately separate the calls in trace mode. Keep
        // the normal-view fold choices while the user inspects those records.
        if self.trace {
            return;
        }
        let old_groups = std::mem::take(&mut self.groups);
        let old_membership = std::mem::take(&mut self.membership);
        let mut start = 0;
        while start < self.source_order.len() {
            if !self.activity_member(&self.source_order[start]) {
                start += 1;
                continue;
            }
            let turn = &self.source_order[start].turn;
            let mut end = start + 1;
            while end < self.source_order.len()
                && self.source_order[end].turn == *turn
                && self.activity_member(&self.source_order[end])
            {
                end += 1;
            }
            if end - start >= 2 {
                let members = self.source_order[start..end].to_vec();
                // Reuse an overlapping group when older history is prepended.
                // A split may reuse an old identity only once.
                let key = members
                    .iter()
                    .filter_map(|key| old_membership.get(key))
                    .find(|key| !self.groups.contains_key(*key))
                    .cloned()
                    .unwrap_or_else(|| {
                        members
                            .iter()
                            .map(MessageKey::group)
                            .find(|key| !self.groups.contains_key(key))
                            .expect("unclaimed group identity")
                    });
                let reads = members
                    .iter()
                    .filter(|key| self.blocks[*key].activity == Some(Activity::Read))
                    .count();
                let searches = members.len() - reads;
                let pending = members
                    .iter()
                    .filter(|key| self.blocks[*key].kind == Kind::Tool(State::Pending))
                    .count();
                self.upsert(
                    key.clone(),
                    Revision::Group {
                        reads,
                        searches,
                        pending,
                    },
                    Kind::Activity,
                    || {
                        let mut parts = Vec::new();
                        for (label, count) in [
                            ("tool-group-reads", reads),
                            ("tool-group-searches", searches),
                        ] {
                            if count > 0 {
                                parts.push(i18n.format(label, &[("count", &count.to_string())]));
                            }
                        }
                        let summary = parts.join(" · ");
                        (if pending == 0 {
                            summary
                        } else {
                            i18n.format(
                                "tool-group-pending",
                                &[("summary", &summary), ("count", &pending.to_string())],
                            )
                        })
                        .into()
                    },
                );
                for member in &members {
                    self.membership.insert(member.clone(), key.clone());
                }
                self.groups.insert(key, members);
            }
            start = end;
        }
        // If a group dissolves, keep reading its surviving real tool, not an
        // unrelated message that happens to occupy the old screen coordinate.
        if let Some(anchor) = &mut self.anchor
            && anchor.key.part == Part::Activity
            && !self.groups.contains_key(&anchor.key)
            && let Some(member) = old_groups.get(&anchor.key).and_then(|members| {
                members
                    .iter()
                    .find(|member| self.source_order.contains(member))
            })
        {
            anchor.key = member.clone();
            anchor.source = 0;
        }
        if let Some(key) = &self.selected
            && key.part == Part::Activity
            && !self.groups.contains_key(key)
        {
            self.selected = old_groups
                .get(key)
                .and_then(|members| {
                    members
                        .iter()
                        .find(|member| self.source_order.contains(member))
                })
                .cloned();
        }
    }

    pub(super) fn arrange_groups(&mut self) {
        self.order.clear();
        for key in &self.source_order {
            let indent = if !self.trace && self.membership.contains_key(key) {
                2
            } else {
                0
            };
            let block = self.blocks.get_mut(key).unwrap();
            if block.indent != indent {
                block.indent = indent;
                block.layout = None;
            }
            if !self.trace
                && let Some(group) = self.membership.get(key)
            {
                if self.groups[group].first() == Some(key) {
                    self.order.push(group.clone());
                }
                // Explicitly opened members stay inspectable even with the
                // surrounding activity collapsed; output never resets folds.
                if self.blocks[group].folded && self.blocks[key].folded {
                    continue;
                }
            }
            self.order.push(key.clone());
        }
        self.selected = self.selected.as_ref().and_then(|key| self.visible_key(key));
        if let Some(anchor) = &mut self.anchor
            && !self.order.contains(&anchor.key)
        {
            if self.trace && anchor.key.part == Part::Activity {
                if let Some(member) = self
                    .groups
                    .get(&anchor.key)
                    .and_then(|members| members.first())
                {
                    anchor.key = member.clone();
                    anchor.source = 0;
                }
            } else if let Some(group) = self.membership.get(&anchor.key) {
                anchor.key = group.clone();
                anchor.source = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Locale, LocalePreference};
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    fn call(id: &str, name: &str) -> Value {
        json!({"type":"tool_call","turnId":"turn","id":id,"toolName":name,
            "origin":"code_mode","args":{"path":format!("{id}.rs")}})
    }
    fn result(id: &str, error: bool) -> Value {
        json!({"type":"tool_result","turnId":"turn","id":format!("result-{id}"),
            "toolUseId":id,"origin":"code_mode","isError":error,
            "content":{"kind":"text","text":if error {"read denied"} else {"real tool output"}}})
    }
    fn draw(view: &mut Transcript, width: u16, ascii: bool) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
        terminal
            .draw(|frame| {
                view.draw(frame, frame.area(), ascii).unwrap();
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn activity_folds_keep_individual_choices_identity_and_attention_across_history_and_trace() {
        let i18n = I18n::new(LocalePreference::Explicit(Locale::En), Locale::En);
        let mut rows = BTreeMap::from([
            (10, call("a", "Read")),
            (20, call("b", "Glob")),
            (30, call("c", "Read")),
            (40, call("edit", "Edit")),
            (50, call("d", "Read")),
            (60, call("e", "Grep")),
            (70, call("failed", "Read")),
            (80, call("waiting", "Read")),
            (90, call("mcp", "mcp__server__Read")),
            (110, result("failed", true)),
        ]);
        let mut view = Transcript::default();
        view.waiting.insert(("turn".into(), "waiting".into()));
        view.sync(&rows, &[], 0, &i18n, false);
        assert_eq!(
            view.groups.len(),
            2,
            "edits, errors, elicitation and unknown tools are boundaries"
        );
        let group = view.order[0].clone();
        let members = view.groups[&group].clone();
        let first = members[0].clone();
        let screen = draw(&mut view, 80, false);
        assert!(screen.contains("Read × 2 · Search × 1 · Awaiting 3 results"));
        assert!(screen.contains("read denied") && screen.contains("Waiting for you"));
        assert!(!screen.contains("a.rs"));
        assert_eq!(view.first_visible(), Some(group.clone()));
        let builds = view.builds;
        assert_eq!(draw(&mut view, 80, false), screen);
        assert_eq!(
            view.builds, builds,
            "unchanged groups reuse their cached layout"
        );
        view.toggle(&group);
        assert!(draw(&mut view, 80, false).contains("  ▸ Read · a.rs"));
        assert!(view.order.contains(&first));
        view.toggle(&first);
        assert!(draw(&mut view, 80, false).contains("Arguments"));
        view.toggle(&group);
        let screen = draw(&mut view, 80, false);
        assert!(
            screen.contains("a.rs") && !screen.contains("b.rs"),
            "manual detail stays open inside a folded group"
        );
        for (sequence, id) in [(120, "b"), (130, "c"), (140, "a")] {
            rows.insert(sequence, result(id, false));
        }
        view.sync(&rows, &[], 0, &i18n, false);
        assert!(!view.blocks[&group].text.contains("Awaiting"));
        assert!(!view.folded(&first));
        assert!(draw(&mut view, 80, false).contains("real tool output"));
        // Prepending a page must not replace the already manipulated group ID.
        rows.insert(5, call("older", "Read"));
        view.sync(&rows, &[], 0, &i18n, false);
        assert_eq!(view.order[0], group);
        assert_eq!(view.groups[&group].len(), 4);
        view.toggle(&group);
        draw(&mut view, 80, false);
        let header_row = view.starts[0] - view.top;
        rows.insert(15, json!({"type":"tool_call","turnId":"turn","id":"exec",
            "toolName":"exec","origin":"provider","args":{"code":"await tools.Read({path:'b.rs'})"}}));
        view.trace = true;
        view.sync(&rows, &[], 0, &i18n, false);
        assert!(!view.order.iter().any(|key| key.part == Part::Activity));
        assert!(draw(&mut view, 80, false).contains("exec"));
        assert_eq!(view.blocks[&first].indent, 0);
        let saved = view.take_reading();
        view = Transcript::resume(saved);
        view.sync(&rows, &[], 0, &i18n, false);
        assert!(!view.folded(&first));
        assert!(draw(&mut view, 80, false).contains("exec"));
        view.trace = false;
        view.sync(&rows, &[], 0, &i18n, false);
        assert!(!view.folded(&group) && !view.folded(&first));
        assert_eq!(view.blocks[&first].indent, 2);
        draw(&mut view, 80, false);
        assert_eq!(view.starts[0] - view.top, header_row);
        for locale in Locale::ALL {
            let i18n = I18n::new(LocalePreference::Explicit(locale), locale);
            view.invalidate_labels();
            view.sync(&rows, &[], 0, &i18n, true);
            for width in [1, 12, 80] {
                assert!(!draw(&mut view, width, true).contains('▸'));
            }
            assert!(i18n.diagnostics().is_empty());
            assert!(!view.folded(&group) && !view.folded(&first));
        }
        // A newly waiting member becomes an independent visible card even if
        // the group was closed; group splits never consume it.
        view.toggle(&group);
        view.waiting.insert(("turn".into(), "older".into()));
        view.sync(&rows, &[], 0, &i18n, false);
        assert_eq!(view.tool_status(&view.order[0]), Some("tool-waiting"));
        assert_eq!(view.groups.len(), 2);
        // Singletons and adjacent calls in different Turns never form a group.
        let mut foreign = call("foreign", "Read");
        foreign["turnId"] = json!("another-turn");
        let rows = BTreeMap::from([(1, call("only", "Read")), (2, foreign)]);
        view.sync(&rows, &[], 0, &i18n, false);
        assert!(view.groups.is_empty());
        assert_eq!(view.order.len(), 2);
    }
}
