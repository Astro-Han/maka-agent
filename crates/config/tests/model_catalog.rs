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

use maka_config::model_catalog::resolve;
use maka_runtime::configuration::{ConnectionCatalogEntry, ModelOverride};
use maka_runtime::execution::ThinkingLevel::{High, Low};
use serde_json::json;

fn connection() -> ConnectionCatalogEntry {
    serde_json::from_value(json!({
        "connectionId":"account","revision":1,"slug":"account","name":"Account",
        "provider":{"packageId":"example.account","entryId":"provider","scope":"profile","name":"api"},
        "configuration":{"baseUrl":"https://example.invalid/v1"},
        "enabled":true,"enabledModelIds":["reported","manual"],"modelSource":"fetched",
        "models":[{
            "id":"reported","thinkingLevels":["low","high"],"contextWindow":16000,"inputLimit":12000,
            "capabilities":{"chat":true,"vision":false,"webSearch":false}
        }]
    })).unwrap()
}

#[test]
fn catalog_projects_account_facts_and_overrides_without_vendor_or_plugin_lookup() {
    let mut row = connection();
    let catalog = resolve(&row, Some("reported")).unwrap();
    assert_eq!(
        catalog
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["reported", "manual"]
    );
    assert_eq!(catalog[0].thinking_levels, [Low, High]);
    assert_eq!(catalog[0].capabilities.web_search, Some(false));
    assert_eq!(catalog[1].capabilities.web_search, None);
    row.model_overrides = Some(
        serde_json::from_value(json!({
            "reported":{
                "thinkingLevels":["high"],"defaultThinkingLevel":"high","vision":true,
                "contextWindow":14000,"inputLimit":10000,"compactionThreshold":9000,
                "capabilities":{"webSearch":true}
            },
            "disabled":{"displayName":"Remembered"}
        }))
        .unwrap(),
    );
    let catalog = resolve(&row, Some("reported")).unwrap();
    let model = &catalog[0];
    assert_eq!(model.thinking_levels, [High]);
    assert_eq!(model.default_thinking_level, Some(High));
    assert_eq!(model.context_window, Some(14000));
    assert_eq!(model.default_context_window, Some(16000));
    assert_eq!(model.input_limit, Some(10000));
    assert_eq!(model.default_input_limit, Some(12000));
    assert_eq!(model.compaction_threshold, Some(9000));
    assert!(model.supports_vision);
    assert_eq!(model.default_supports_vision, Some(false));
    assert_eq!(model.capabilities.web_search, Some(true));
    assert_eq!(catalog[2].id, "disabled");
    assert_eq!(
        row.models[0].context_window,
        Some(16000),
        "projection does not rewrite inventory"
    );
}

#[test]
fn empty_account_choices_and_nonchat_facts_are_not_replaced_by_name_heuristics() {
    let mut row = connection();
    row.models[0].id = "gpt-future".into();
    row.models[0].thinking_levels = Some(vec![]);
    row.enabled_model_ids = vec!["gpt-future".into()];
    row.models[0].capabilities.as_mut().unwrap().chat = Some(false);
    let model = resolve(&row, Some("gpt-future")).unwrap().remove(0);
    assert!(model.thinking_levels.is_empty());
    assert!(!model.can_use_as_chat_default);
    row.model_overrides = Some(std::collections::BTreeMap::from([(
        "gpt-future".into(),
        ModelOverride {
            thinking_levels: Some(vec![High]),
            ..Default::default()
        },
    )]));
    assert_eq!(resolve(&row, None).unwrap()[0].thinking_levels, [High]);
}
