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

use super::{Composition, Entry, Operation, Scope};
use crate::Error;

impl Composition {
    /// An invalid batch cannot partially mutate the caller's desired state.
    pub fn apply(&self, operations: &[Operation]) -> Result<Self, Error> {
        self.validate()?;
        if operations.len() > 4096 {
            return Err(Error::Invalid(
                "operation batch exceeds 4096 entries".into(),
            ));
        }
        let mut next = self.clone();
        for operation in operations {
            next.apply_one(operation)?;
            next.validate()?;
        }
        Ok(next)
    }

    pub fn find(&self, id: &str) -> Option<(&Scope, &Entry)> {
        self.roots
            .iter()
            .find_map(|(scope, entries)| find(entries, id).map(|entry| (scope, entry)))
    }

    fn apply_one(&mut self, operation: &Operation) -> Result<(), Error> {
        match operation {
            Operation::Insert {
                root_id,
                parent_id,
                entry,
                position,
            } => {
                let parent_scope = parent_id.as_deref().map(|id| self.scope(id)).transpose()?;
                let root = root_id.clone().or(parent_scope.clone()).unwrap_or_default();
                if parent_scope.as_ref().is_some_and(|scope| scope != &root) {
                    return Err(Error::CrossRoot);
                }
                let siblings = self.siblings(&root, parent_id.as_deref())?;
                siblings.insert(
                    position.unwrap_or(usize::MAX).min(siblings.len()),
                    entry.clone(),
                );
            }
            Operation::Update { entry_id, patch } => {
                let scope = self.scope(entry_id)?;
                let entry = find_mut(self.roots.get_mut(&scope).expect("located root"), entry_id)
                    .expect("located entry");
                patch.apply(entry);
            }
            Operation::Move {
                entry_id,
                parent_id,
                position,
            } => {
                let scope = self.scope(entry_id)?;
                if let Some(parent) = parent_id {
                    if self.scope(parent)? != scope {
                        return Err(Error::CrossRoot);
                    }
                    let entry = self.find(entry_id).expect("located entry").1;
                    if entry.id == *parent || find(&entry.children, parent).is_some() {
                        return Err(Error::DependencyCycle);
                    }
                }
                let entry = take(self.roots.get_mut(&scope).expect("located root"), entry_id)
                    .expect("located entry");
                let siblings = self.siblings(&scope, parent_id.as_deref())?;
                siblings.insert(position.unwrap_or(usize::MAX).min(siblings.len()), entry);
            }
            Operation::Remove { entry_id } => {
                let scope = self.scope(entry_id)?;
                take(self.roots.get_mut(&scope).expect("located root"), entry_id);
            }
        }
        Ok(())
    }

    fn scope(&self, id: &str) -> Result<Scope, Error> {
        self.find(id)
            .map(|(scope, _)| scope.clone())
            .ok_or_else(|| Error::MissingEntry(id.to_owned()))
    }

    fn siblings(&mut self, scope: &Scope, parent: Option<&str>) -> Result<&mut Vec<Entry>, Error> {
        let entries = self.roots.entry(scope.clone()).or_default();
        match parent {
            Some(parent) => find_mut(entries, parent)
                .map(|entry| &mut entry.children)
                .ok_or_else(|| Error::MissingEntry(parent.into())),
            None => Ok(entries),
        }
    }
}

fn find<'a>(entries: &'a [Entry], id: &str) -> Option<&'a Entry> {
    entries.iter().find_map(|entry| {
        if entry.id == id {
            Some(entry)
        } else {
            find(&entry.children, id)
        }
    })
}

fn find_mut<'a>(entries: &'a mut [Entry], id: &str) -> Option<&'a mut Entry> {
    entries.iter_mut().find_map(|entry| {
        if entry.id == id {
            Some(entry)
        } else {
            find_mut(&mut entry.children, id)
        }
    })
}

fn take(entries: &mut Vec<Entry>, id: &str) -> Option<Entry> {
    if let Some(index) = entries.iter().position(|entry| entry.id == id) {
        return Some(entries.remove(index));
    }
    entries
        .iter_mut()
        .find_map(|entry| take(&mut entry.children, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ops(value: serde_json::Value) -> Vec<Operation> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn edits_are_atomic_keep_root_ownership_and_reject_recursive_moves() {
        let tree = Composition::default()
            .apply(&ops(json!([
                {"type":"insert","entry":{"id":"parent","children":[{"id":"child"}]}},
                {"type":"insert","rootId":"session:s","entry":{"id":"other"}}
            ])))
            .unwrap();
        assert_eq!(
            tree.apply(&ops(json!([
                {"type":"move","entryId":"parent","parentId":"child"}
            ]))),
            Err(Error::DependencyCycle)
        );
        assert_eq!(
            tree.apply(&ops(json!([
                {"type":"move","entryId":"child","parentId":"other"}
            ]))),
            Err(Error::CrossRoot)
        );
        assert!(
            tree.apply(&ops(json!([
                {"type":"remove","entryId":"child"},
                {"type":"insert","entry":{"id":"other"}}
            ])))
            .is_err()
        );
        assert_eq!(tree.find("child").unwrap().0, &Scope::Profile);
        let changed = tree
            .apply(&ops(json!([
                {"type":"move","entryId":"child","position":0},
                {"type":"remove","entryId":"parent"}
            ])))
            .unwrap();
        assert_eq!(changed.roots[&Scope::Profile][0].id, "child");
        assert!(changed.find("parent").is_none());
    }
}
