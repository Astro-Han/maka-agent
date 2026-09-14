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

use super::*;

#[tokio::test]
async fn registry_defaults_and_explicit_defaults_remain_canonical_after_save_and_edit() {
    let temp = tempfile::tempdir().unwrap();
    let namespaces = RootNamespaces {
        ownership: temp.path().join("owners"),
        control: temp.path().join("control"),
    };
    let owner = Arc::new(RootOwner::create(&temp.path().join("root"), &namespaces).unwrap());
    let store = Arc::new(ConfigurationStore::for_root(owner).await.unwrap());
    // Independent compatible identities use the registry contract, not a Rust whitelist.
    for provider in ["openai", "anthropic", "openrouter", "deepseek"] {
        let endpoint = validation::provider_default_base_url(provider).unwrap();
        let effective_endpoint = validation::normalize_base_url(Some(endpoint), None)
            .unwrap()
            .unwrap();
        for supplied_endpoint in [None, Some(endpoint.to_owned())] {
            let ticket = prepare(
                &store,
                OnboardingInput {
                    target: OnboardingTarget::Create {
                        provider_type: provider.into(),
                        slug: None,
                        name: None,
                    },
                    api_key: Some("key".into()),
                    base_url: supplied_endpoint,
                },
            )
            .await;
            assert_eq!(ticket.endpoint(), effective_endpoint);
            assert_eq!(ticket.connection().base_url, None);
            let OnboardingSaveResult::Saved { connection } = ticket
                .complete(vec![ModelInfo::new("one")], vec![], 1)
                .await
                .unwrap()
            else {
                panic!("saved");
            };
            let ticket = prepare(
                &store,
                OnboardingInput {
                    target: OnboardingTarget::Existing {
                        connection_id: connection.connection_id.clone(),
                    },
                    api_key: None,
                    base_url: None,
                },
            )
            .await;
            assert_eq!(ticket.endpoint(), effective_endpoint);
            ticket
                .complete(vec![ModelInfo::new("one")], vec![], 2)
                .await
                .unwrap();
            let row = store
                .catalog()
                .await
                .unwrap()
                .connections
                .into_iter()
                .find(|r| r.connection_id == connection.connection_id)
                .unwrap();
            assert_eq!(row.base_url, None);
            assert_eq!(
                row.name,
                maka_config::model_catalog::provider_facts(provider)
                    .unwrap()
                    .label
            );
            assert_eq!(row.revision, 2);
        }
    }
    store.shutdown().await.unwrap();
}
