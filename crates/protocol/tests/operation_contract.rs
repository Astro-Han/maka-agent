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

use maka_protocol::Operation;
use serde_json::{Map, Value, json};

#[test]
fn native_operation_inventory_and_execution_targets_preserve_the_wire_contract() {
    let mut operations = Map::new();
    for &operation in Operation::ALL {
        let name = operation.as_str();
        assert_eq!(name.parse::<Operation>().unwrap(), operation);
        assert_eq!(serde_json::to_value(operation).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<Operation>(json!(name)).unwrap(),
            operation
        );
        assert!(
            operations
                .insert(
                    name.to_owned(),
                    json!({"mode": operation.mode(), "availability": operation.availability(), "unavailableError": operation.unavailable_error()}),
                )
                .is_none(),
            "Duplicate operation: {name}"
        );
    }

    assert_eq!(operations["turn.batch.start"], operations["turn.start"]);
    for (index, case) in execution_targets().iter().enumerate() {
        assert_eq!(case["decoded"].is_object(), index < 3, "{case}");
    }
    for page in provider_pages() {
        let decoded: maka_plugins::provider::catalog::Page =
            serde_json::from_value(page.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), page);
    }
}

fn provider_pages() -> Vec<Value> {
    use maka_plugins::provider::{
        Descriptor, Identity,
        authentication::Method,
        catalog::{Entry, Page},
    };
    let descriptor = Descriptor {
        label: "模型账户".into(),
        configuration_schema: json!({"type":"object"}),
        configuration_defaults: json!({}),
        anonymous: false,
        authentication: vec![Method {
            id: "key".into(),
            label: "API key".into(),
            input_schema: json!({"type":"object"}),
            interactive: false,
        }],
        discovery: true,
    };
    descriptor.validate().unwrap();
    let identity = Identity {
        package_id: "example.provider".into(),
        entry_id: "example.provider".into(),
        scope: maka_runtime::scope::Scope::Profile,
        name: "provider".into(),
    };
    identity.validate().unwrap();
    [
        Page::Page {
            revision: 7,
            entries: vec![Entry {
                identity,
                descriptor,
            }],
            next: Some("provider".into()),
        },
        Page::RevisionChanged { revision: 8 },
    ]
    .into_iter()
    .map(|page| serde_json::to_value(page).unwrap())
    .collect()
}

fn execution_targets() -> Vec<Value> {
    let mut cases = Vec::new();
    for target in [
        json!({"modelTarget":{"kind":"default"}}),
        json!({"executorId":"Agent.v1:fixture"}),
        json!({"executorId":"a".repeat(128)}),
        json!({"executorId":"a".repeat(129)}),
        json!({"executorId":"1agent"}),
        json!({"executorId":null}),
        json!({"executorId":"Agent", "modelTarget":{"kind":"default"}}),
        json!({"executorId":"Agent", "unexpected":true}),
        json!({}),
    ] {
        let mut input =
            json!({"sessionId":"session", "workspace":{"kind":"host_path", "path":"/work"}});
        input
            .as_object_mut()
            .unwrap()
            .extend(target.as_object().unwrap().clone());
        let decoded = maka_protocol::session::decode_session_create_input(&input)
            .ok()
            .map(|value| serde_json::to_value(value).unwrap());
        cases.push(json!({"operation":Operation::SessionCreate, "input":input, "decoded":decoded}));
    }
    cases
}
