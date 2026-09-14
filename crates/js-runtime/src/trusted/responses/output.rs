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

use super::cache::CACHE_LIMIT;
use crate::trusted::budget;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Only finalized provider items are replayable. Added items track unfinished
/// slots without retaining their partial contents. This is one request's cache.
#[derive(Default)]
pub(super) struct Output(BTreeMap<u64, Item>);

enum Item {
    Started(String),
    Done(Value),
}

impl Output {
    pub fn observe(
        &mut self,
        event: &mut Value,
        permit: &mut OwnedSemaphorePermit,
        budget: &Arc<Semaphore>,
    ) -> Option<()> {
        let done = match event["type"].as_str() {
            Some("response.output_item.added") => false,
            Some("response.output_item.done") => true,
            _ => return Some(()),
        };
        let index = event["output_index"].as_u64()?;
        let item = &event["item"];
        let id = item["id"].as_str().filter(|id| !id.is_empty())?;
        item["type"].as_str()?;
        match self.0.get(&index) {
            Some(Item::Started(previous)) if done && previous == id => {}
            Some(_) => return None,
            None => {}
        }
        // Charge before retaining the item. Include a small slot allowance;
        // old Started storage stays conservatively reserved until completion.
        let bytes = if done {
            budget::bytes(item, CACHE_LIMIT).ok()?
        } else {
            budget::bytes(&id, CACHE_LIMIT).ok()?
        }
        .checked_add(64)?;
        (permit.num_permits().checked_add(bytes as usize)? <= CACHE_LIMIT as usize).then_some(())?;
        permit.merge(budget.clone().try_acquire_many_owned(bytes).ok()?);
        let item = if done {
            Item::Done(event["item"].take())
        } else {
            Item::Started(id.to_owned())
        };
        self.0.insert(index, item);
        Some(())
    }

    pub fn finish(self) -> Option<Vec<Value>> {
        if self.0.is_empty() {
            return None;
        }
        self.0
            .into_iter()
            .enumerate()
            .map(|(expected, (index, item))| {
                if index != expected as u64 {
                    return None;
                }
                match item {
                    Item::Done(item) => Some(item),
                    Item::Started(_) => None,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::cache::Prepared;
    use super::*;
    use serde_json::json;

    fn event(index: u64, id: &str, done: bool) -> Value {
        json!({"type": if done {"response.output_item.done"} else {"response.output_item.added"},
            "output_index":index, "item":{"type":"message","id":id,"role":"assistant",
            "status":"completed","phase":"final_answer",
            "content":[{"type":"output_text","text":"cached","annotations":[]}]}})
    }

    #[test]
    fn finalized_items_are_ordered_complete_and_budgeted_without_duplicate_terminal_output() {
        let budget = Arc::new(Semaphore::new(4096));
        let body = json!({"input":[{"role":"user","content":"first"}]});
        let original = [
            event(0, "a", true)["item"].clone(),
            event(1, "b", true)["item"].clone(),
        ];
        for full_terminal in [false, true] {
            let mut baseline = Prepared::new(body.clone(), None)
                .unwrap()
                .cache(&budget)
                .unwrap();
            for (index, id, done) in [
                (0, "a", false),
                (1, "b", false),
                (1, "b", true),
                (0, "a", true),
            ] {
                baseline
                    .observe(&mut event(index, id, done), &budget)
                    .unwrap();
            }
            let mut terminal =
                json!({"id":"resp","output":if full_terminal {json!(original)} else {json!([])}});
            let baseline = baseline.complete(&mut terminal, &budget).unwrap();
            assert_eq!(baseline.response_id.as_deref(), Some("resp"));
            assert_eq!(&baseline.body["input"].as_array().unwrap()[1..], &original);
            assert_eq!(baseline.body["input"].as_array().unwrap().len(), 3);
            drop(baseline);
            assert_eq!(budget.available_permits(), 4096);
        }
        // Partial, duplicate, misidentified and gapped output must never be
        // advertised to semantic confirmation as a reconstructable response.
        for events in [
            vec![event(0, "a", false)],
            vec![event(1, "a", true)],
            vec![event(0, "a", true), event(0, "a", true)],
            vec![event(0, "a", false), event(0, "b", true)],
            vec![event(0, "a", true), event(1, "b", false)],
        ] {
            let mut baseline = Prepared::new(body.clone(), None).unwrap().cache(&budget);
            for mut event in events {
                baseline = baseline.and_then(|mut value| {
                    value.observe(&mut event, &budget)?;
                    Some(value)
                });
            }
            let completed = baseline
                .and_then(|value| value.complete(&mut json!({"id":"resp","output":[]}), &budget));
            assert!(completed.is_none());
            assert_eq!(budget.available_permits(), 4096);
        }
        let mut baseline = Prepared::new(body, None).unwrap().cache(&budget).unwrap();
        baseline
            .observe(&mut event(0, "a", false), &budget)
            .unwrap();
        let remaining = budget.available_permits() as u32;
        let competing = budget.clone().try_acquire_many_owned(remaining).unwrap();
        let mut done = event(0, "a", true);
        assert!(baseline.observe(&mut done, &budget).is_none());
        assert_eq!(
            done["item"]["id"], "a",
            "failed reservation must not move the raw item"
        );
        drop(baseline);
        drop(competing);
        assert_eq!(budget.available_permits(), 4096);
    }
}
