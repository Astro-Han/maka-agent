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

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Composition, Operation};
use crate::{Error, identifier};

/// Durable authority consists of inputs, never a second copy of the derived tree.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ledger {
    pub generation: u64,
    pub package_layers: Vec<String>,
    pub overlays: Vec<Operation>,
}

impl Ledger {
    pub fn project(&self, layers: &BTreeMap<String, Vec<Operation>>) -> Result<Composition, Error> {
        if self.generation > (1_u64 << 53) - 1 || self.package_layers.len() > 256 {
            return Err(Error::Invalid(
                "invalid composition generation or layer count".into(),
            ));
        }
        let mut packages = BTreeSet::new();
        let mut composition = Composition::default();
        for package in &self.package_layers {
            identifier(package)?;
            if !packages.insert(package) {
                return Err(Error::Invalid(format!(
                    "duplicate package layer: {package}"
                )));
            }
            let layer = layers
                .get(package)
                .ok_or_else(|| Error::Invalid(format!("missing package layer: {package}")))?;
            composition = composition.apply(layer)?;
        }
        // Only request batches are bounded at 4096; the durable history is not.
        for chunk in self.overlays.chunks(4096) {
            composition = composition.apply(chunk)?;
        }
        Ok(composition)
    }

    /// Coalesce writes only while no structural operation can change their target.
    /// Keep operations relative to package layers, including their ordering.
    pub fn extend(&mut self, operations: &[Operation]) {
        self.overlays.extend_from_slice(operations);
        let mut compacted: Vec<Operation> = Vec::with_capacity(self.overlays.len());
        let mut updates: BTreeMap<String, usize> = BTreeMap::new();
        for operation in self.overlays.drain(..) {
            if let Operation::Update { entry_id, patch } = &operation {
                if let Some(index) = updates.get(entry_id) {
                    let Operation::Update {
                        patch: previous, ..
                    } = &mut compacted[*index]
                    else {
                        unreachable!("update index refers to an update")
                    };
                    previous.merge(patch);
                    continue;
                }
                updates.insert(entry_id.clone(), compacted.len());
            } else {
                updates.clear();
            }
            compacted.push(operation);
        }
        self.overlays = compacted;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn operations(value: serde_json::Value) -> Vec<Operation> {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn overlay_compaction_preserves_null_and_future_package_defaults() {
        let mut ledger = Ledger {
            generation: 0,
            package_layers: vec!["graph".into()],
            overlays: vec![],
        };
        let layer = operations(json!([
            {"type":"insert","entry":{"id":"graph","config":{"version":1}}}
        ]));
        let mut layers = BTreeMap::from([("graph".into(), layer)]);
        for disabled in [true, false].into_iter().cycle().take(5000) {
            ledger.extend(&operations(json!([
                {"type":"update","entryId":"graph","patch":{"disabled":disabled}},
                {"type":"update","entryId":"graph","patch":{"config":null}}
            ])));
        }
        assert_eq!(ledger.overlays.len(), 1);
        assert!(
            ledger
                .project(&layers)
                .unwrap()
                .find("graph")
                .unwrap()
                .1
                .config
                .is_null()
        );
        layers.insert(
            "graph".into(),
            operations(json!([
                {"type":"insert","entry":{"id":"graph","config":{"version":2},
                    "children":[{"id":"new-default"}]}}
            ])),
        );
        let projected = ledger.project(&layers).unwrap();
        assert!(projected.find("new-default").is_some());
        assert!(!projected.find("graph").unwrap().1.disabled);
        assert!(projected.find("graph").unwrap().1.config.is_null());
        let encoded = serde_json::to_value(&ledger).unwrap();
        assert!(
            encoded["overlays"][0]["patch"]
                .as_object()
                .unwrap()
                .contains_key("config")
        );
        let recovered: Ledger = serde_json::from_value(encoded).unwrap();
        assert_eq!(recovered.project(&layers).unwrap(), projected);
    }
}
