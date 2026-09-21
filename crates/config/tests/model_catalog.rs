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

use maka_config::model_catalog::{provider_facts, resolve};
use maka_runtime::configuration::{ConnectionCatalogEntry, validation::provider_auth_kind};
use serde_json::{Value, json};

#[test]
fn search_capability_uses_provider_defaults_not_model_name_guesses() {
    for (provider, model, declared, overridden, expected) in [
        ("openai", "future-model", None, None, Some(true)),
        ("openai-codex", "future-model", None, None, Some(true)),
        ("openai", "future-model", Some(false), None, Some(false)),
        (
            "openai",
            "future-model",
            Some(true),
            Some(false),
            Some(false),
        ),
        (
            "openai-responses-compatible",
            "gpt-5.6-luna",
            None,
            None,
            None,
        ),
        (
            "anthropic-compatible",
            "claude-sonnet-4-6",
            None,
            None,
            None,
        ),
        (
            "openai-responses-compatible",
            "local-model",
            Some(true),
            None,
            Some(true),
        ),
        (
            "anthropic-compatible",
            "local-model",
            None,
            Some(true),
            Some(true),
        ),
    ] {
        let mut wire = json!({
            "connectionId":"test", "revision":1, "slug":"test", "name":"Test",
            "providerType":provider, "enabled":true, "enabledModelIds":[model],
            "modelSource":"fetched", "models":[{"id":model}]
        });
        if let Some(value) = declared {
            wire["models"][0]["capabilities"] = json!({"webSearch":value});
        }
        if let Some(value) = overridden {
            wire["modelOverrides"] = json!({model:{"capabilities":{"webSearch":value}}});
        }
        let row = serde_json::from_value(wire).unwrap();
        let catalog = resolve(&row, Some(model)).unwrap();
        assert_eq!(
            catalog[0].capabilities.web_search, expected,
            "{provider}/{model}"
        );
    }
}

#[test]
fn source_catalog_differential() {
    let facts: Value = serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/catalog-facts.json"
    )))
    .unwrap();
    for (provider, source) in facts.as_object().unwrap() {
        let typed = provider_facts(provider).unwrap();
        let auth = typed.auth_kind;
        assert_eq!(auth, provider_auth_kind(provider).unwrap(), "{provider}");
        assert_eq!(serde_json::to_value(auth).unwrap(), source["authKind"]);
        for (field, actual) in [
            (
                "runtimeAdapter",
                serde_json::to_value(&typed.runtime_adapter).unwrap(),
            ),
            (
                "protocolAdapters",
                serde_json::to_value(&typed.protocol_adapters).unwrap(),
            ),
            (
                "modelDiscovery",
                serde_json::to_value(&typed.model_discovery).unwrap(),
            ),
        ] {
            assert_eq!(actual, source[field], "{provider}.{field}");
        }
        for (model, facts) in &typed.models {
            for (field, actual) in [
                (
                    "runtimeOverride",
                    serde_json::to_value(&facts.runtime_override).unwrap(),
                ),
                ("metadata", serde_json::to_value(&facts.metadata).unwrap()),
                ("entry", serde_json::to_value(&facts.entry).unwrap()),
            ] {
                assert_eq!(
                    actual, source["models"][model][field],
                    "{provider}.{model}.{field}"
                );
            }
        }
    }
    let fixtures: Vec<Value> = serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/catalog-oracle.json"
    )))
    .unwrap();
    for fixture in fixtures {
        let connection = &fixture["connection"];
        let mut wire = json!({
            "connectionId": "test-id", "revision": 1, "slug": "test", "name": "Test",
            "providerType": connection["providerType"], "enabled": true,
            "enabledModelIds": [], "models": []
        });
        for field in ["models", "enabledModelIds", "modelOverrides", "modelSource"] {
            if let Some(value) = connection.get(field) {
                wire[field] = value.clone();
            }
        }
        let row: ConnectionCatalogEntry = serde_json::from_value(wire).unwrap();
        if let Some(models) = connection.get("models") {
            assert_eq!(serde_json::to_value(&row.models).unwrap(), *models);
        }
        let actual = resolve(&row, connection["defaultModel"].as_str()).unwrap();
        let expected = fixture["expected"].as_array().unwrap();
        assert_eq!(actual.len(), expected.len(), "connection: {connection}");
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                *expected,
                "connection: {connection}"
            );
        }
    }
}
