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

use super::connection::pair_with;
use maka_client::{ClientError, RequestFailure};
use maka_protocol::model_provider::{Page, Query, Scope};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn provider_directory_restarts_whole_snapshot_and_bounds_publication_churn() {
    for continuous_churn in [false, true] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move { client.provider_directory(Scope::Profile).await }
        });
        let attempts = if continuous_churn { 8 } else { 2 };
        for attempt in 0..attempts {
            let revision = attempt + 1;
            for after in [None, Some("a")] {
                let frame = tokio::time::timeout(Duration::from_secs(1), reader.read())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(frame["operation"], "model.provider.catalog.query");
                assert_eq!(
                    frame["input"],
                    json!({
                        "scope":"profile", "after":after,
                        "revision":after.map(|_| revision),
                    })
                );
                let changed = after.is_some() && (continuous_churn || attempt == 0);
                let name = if after.is_none() { "a" } else { "b" };
                let result = if changed {
                    json!({"kind":"revision_changed","revision":revision + 1})
                } else {
                    json!({"kind":"page","revision":revision,
                    "next":if after.is_none() { Some("a") } else { None },
                    "entries":[{
                        "identity":{"packageId":"external.provider","entryId":"provider","scope":"profile","name":name},
                        "descriptor":{"label":format!("Publication {revision}"),
                            "configurationSchema":{"type":"object"},"configurationDefaults":{},
                            "authentication":[],"anonymous":true,"discovery":true}
                    }]})
                };
                writer
                    .write(&json!({"requestId":frame["requestId"],
                    "operation":frame["operation"],"ok":true,"result":result}))
                    .await
                    .unwrap();
            }
        }
        let result = request.await.unwrap();
        if continuous_churn {
            assert!(matches!(
                result,
                Err(maka_client::ProviderDirectoryError::Unstable)
            ));
        } else {
            let directory = result.unwrap();
            assert_eq!(directory.revision, 2);
            assert_eq!(
                directory
                    .entries
                    .iter()
                    .map(|entry| entry.identity.name.as_str())
                    .collect::<Vec<_>>(),
                ["a", "b"]
            );
            assert!(
                directory
                    .entries
                    .iter()
                    .all(|entry| entry.descriptor.label == "Publication 2")
            );
        }
        client.disconnect();
    }
}

#[tokio::test]
async fn provider_directory_keeps_scope_revision_and_cursor_bound_to_the_request() {
    for case in 0..6 {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .model_providers(Query {
                        scope: Scope::Session("session".into()),
                        after: Some("a".into()),
                        revision: Some(7),
                    })
                    .await
            }
        });
        let frame = tokio::time::timeout(Duration::from_secs(1), reader.read())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(frame["operation"], "model.provider.catalog.query");
        let mut result = json!({"kind":"page","revision":7,"next":"b","entries":[{
            "identity":{"packageId":"external.provider","entryId":"provider","scope":"profile","name":"b"},
            "descriptor":{"label":"External provider","configurationSchema":{"type":"object"},
                "configurationDefaults":{},"authentication":[],"anonymous":true,"discovery":true}
        }]});
        match case {
            0 => {}
            1 => result = json!({"kind":"revision_changed","revision":8}),
            2 => result["revision"] = json!(8),
            3 => result["entries"][0]["identity"]["scope"] = json!("session:other"),
            4 => {
                result["entries"][0]["identity"]["name"] = json!("a");
                result["next"] = json!("a");
            }
            5 => result["next"] = json!("missing"),
            _ => unreachable!(),
        }
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        let reply = request.await.unwrap();
        match case {
            0 => assert!(matches!(reply, Ok(Page::Page { .. }))),
            1 => assert!(matches!(reply, Ok(Page::RevisionChanged { revision: 8 }))),
            _ => assert!(matches!(
                reply,
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            )),
        }
        client.disconnect();
    }
}
