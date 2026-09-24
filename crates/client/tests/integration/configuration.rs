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
use maka_protocol::configuration::{
    ConnectionCatalogCursor as Cursor, ConnectionCatalogQueryInput as Query,
};
use serde_json::json;
use std::time::Duration;

fn provider(name: &str) -> maka_protocol::oauth::Identity {
    maka_protocol::oauth::Identity {
        package_id: "example.providers".into(),
        entry_id: "example.providers".into(),
        scope: maka_protocol::model_provider::Scope::Profile,
        name: name.into(),
    }
}

#[tokio::test]
async fn connection_test_binds_target_and_model_but_distinguishes_committed_failure() {
    use maka_protocol::connection_effects::ConnectionTestRunInput;
    const ID: &str = "b746eb13-287c-4f3a-8590-dac93c0a1253";
    for (model, connection, test, valid) in [
        (
            None,
            ID,
            json!({"kind":"verified","checkedAt":"2026-09-23T00:00:00Z","modelId":"chosen","latencyMs":12}),
            true,
        ),
        (
            Some("chosen"),
            ID,
            json!({"kind":"verified","checkedAt":"now","modelId":"other","latencyMs":12}),
            false,
        ),
        (
            Some("chosen"),
            ID,
            json!({"kind":"failed","checkedAt":"now","modelId":null,"latencyMs":12,"statusCode":401,"errorClass":"auth"}),
            true,
        ),
        (
            Some("chosen"),
            ID,
            json!({"kind":"failed","checkedAt":"now","modelId":"other","latencyMs":null,"statusCode":null,"errorClass":"network"}),
            false,
        ),
        (
            None,
            "fe26c818-0e6a-47ce-861c-e8c28f053bbd",
            json!({"kind":"verified","checkedAt":"now","modelId":"chosen","latencyMs":12}),
            false,
        ),
    ] {
        let result = json!({"kind":"committed","catalogRevision":30,"connection":{"connectionId":connection,"revision":19},"test":test});
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move {
                client
                    .test_connection(ConnectionTestRunInput {
                        connection_id: ID.into(),
                        model_id: model.map(str::to_owned),
                    })
                    .await
            }
        });
        let frame = tokio::time::timeout(Duration::from_secs(1), reader.read())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(frame["operation"], "connection.test.run");
        assert_eq!(frame["input"], json!({"connectionId":ID,"modelId":model}));
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        if valid {
            assert_eq!(
                serde_json::to_value(request.await.unwrap().unwrap()).unwrap(),
                result
            );
            client.disconnect();
        } else {
            assert!(matches!(
                request.await.unwrap(),
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
        }
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn connection_model_fetch_binds_identity_without_inventing_a_revision_precondition() {
    const ID: &str = "b746eb13-287c-4f3a-8590-dac93c0a1253";
    for (result, valid) in [
        (
            json!({"kind":"committed","catalogRevision":30,"connection":{"connectionId":ID,"revision":19},"modelCount":4,"source":"fetched","fetchedAt":1}),
            true,
        ),
        (
            json!({"kind":"committed","catalogRevision":30,"connection":{"connectionId":"fe26c818-0e6a-47ce-861c-e8c28f053bbd","revision":19},"modelCount":4,"source":"fetched","fetchedAt":1}),
            false,
        ),
        (json!({"kind":"superseded","changed":["credential"]}), true),
        (
            json!({"kind":"rejected","reason":"connection_disabled"}),
            true,
        ),
        (json!({"kind":"failed","errorClass":"auth"}), true),
    ] {
        maka_protocol::connection_effects::decode_connection_model_fetch_result(&result).unwrap();
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move { client.fetch_connection_models(ID).await }
        });
        let frame = tokio::time::timeout(Duration::from_secs(1), reader.read())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(frame["operation"], "connection.models.fetch");
        assert_eq!(frame["input"], json!({"connectionId":ID}));
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        if valid {
            assert_eq!(
                serde_json::to_value(request.await.unwrap().unwrap()).unwrap(),
                result
            );
            client.disconnect();
        } else {
            assert!(matches!(
                request.await.unwrap(),
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
        }
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn default_model_mutations_bind_catalog_revision_and_explicit_null_target() {
    use maka_protocol::{Operation, configuration::*};
    let target = ConnectionTarget {
        connection_id: "b746eb13-287c-4f3a-8590-dac93c0a1253".into(),
        model_id: "model".into(),
    };
    for (clear, result, valid) in [
        (false, json!({"kind":"committed","catalogRevision":8}), true),
        (true, json!({"kind":"committed","catalogRevision":8}), true),
        (
            false,
            json!({"kind":"revision_conflict","expectedRevision":7,"actualRevision":9}),
            true,
        ),
        (
            false,
            json!({"kind":"invalid_default_target","target":target}),
            true,
        ),
        (
            true,
            json!({"kind":"invalid_default_target","target":target}),
            false,
        ),
        (
            false,
            json!({"kind":"committed","catalogRevision":9}),
            false,
        ),
        (
            false,
            json!({"kind":"revision_conflict","expectedRevision":6,"actualRevision":9}),
            false,
        ),
        (
            false,
            json!({"kind":"revision_conflict","expectedRevision":7,"actualRevision":7}),
            false,
        ),
        (
            false,
            json!({"kind":"invalid_default_target","target":{"connectionId":target.connection_id,"modelId":"other"}}),
            false,
        ),
    ] {
        decode_catalog_mutation_result(Operation::ConnectionCatalogSetDefaultTarget, &result)
            .unwrap();
        let input = SetDefaultConnectionTargetInput {
            expected_catalog_revision: 7,
            target: (!clear).then(|| target.clone()),
        };
        let expected = serde_json::to_value(&input).unwrap();
        assert!(
            expected.get("target").is_some(),
            "clear must send explicit null"
        );
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move { client.set_default_model(input).await }
        });
        let frame = tokio::time::timeout(Duration::from_secs(1), reader.read())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(frame["input"], expected);
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        if valid {
            assert_eq!(
                serde_json::to_value(request.await.unwrap().unwrap()).unwrap(),
                result
            );
            client.disconnect();
        } else {
            assert!(matches!(
                request.await.unwrap(),
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
        }
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn connection_catalog_binds_pages_to_requested_revision_and_exact_cursor() {
    let header = json!({"kind":"connection","connectionIndex":0,"connectionId":"b746eb13-287c-4f3a-8590-dac93c0a1253",
        "revision":1,"slug":"fixture","name":"Fixture","provider":provider("api"),"configuration":{},
        "enabled":true,"enabledModelIdCount":1,"modelCount":0,"catalogEntryCount":1});
    let item = json!({"kind":"catalog_entry","connectionIndex":0,"itemIndex":0,
        "entry":{"id":"model","canUseAsChatDefault":true,"isDefault":false,"supportsVision":false,"thinkingLevels":[]}});
    let page = |items| json!({"kind":"page","revision":7,"defaultTarget":null,"connectionCount":1,"items":items,"nextCursor":null});
    let continuation = Query::Continue {
        revision: 7,
        cursor: Cursor::CatalogEntry {
            connection_index: 0,
            item_index: 0,
        },
    };
    let changed = json!({"kind":"revision_changed","expectedRevision":7,"actualRevision":8});
    let mut wrong_revision = page(vec![item.clone()]);
    wrong_revision["revision"] = json!(8);
    let mut skipped = item.clone();
    skipped["itemIndex"] = json!(1);
    for (input, result, valid) in [
        (Query::Start, page(vec![header]), true),
        (continuation.clone(), page(vec![item.clone()]), true),
        (continuation.clone(), changed.clone(), true),
        (Query::Start, page(vec![item]), false),
        (Query::Start, changed, false),
        (continuation.clone(), wrong_revision, false),
        (continuation.clone(), page(vec![skipped]), false),
        (
            continuation.clone(),
            json!({"kind":"revision_changed","expectedRevision":6,"actualRevision":8}),
            false,
        ),
        (
            continuation,
            json!({"kind":"revision_changed","expectedRevision":7,"actualRevision":7}),
            false,
        ),
    ] {
        // Each fixture is independently valid: the rejection must be request/result binding.
        maka_protocol::configuration_pages::decode_catalog_query_result(&result).unwrap();
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move { client.connection_catalog(input).await }
        });
        let frame = tokio::time::timeout(Duration::from_secs(1), reader.read())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        let reply = request.await.unwrap();
        if valid {
            assert_eq!(reply.unwrap(), result);
            client.disconnect();
        } else {
            assert!(matches!(
                reply,
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
        }
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn onboarding_rejects_ambiguous_inventory_and_foreign_saved_connection() {
    use maka_protocol::configuration::onboarding::*;
    for case in 0..3 {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move {
                let input = OnboardingInput {
                    target: maka_protocol::oauth::Target::Create {
                        provider: provider("api"),
                        configuration: json!({"baseUrl":"http://127.0.0.1/v1"}),
                        slug: "wanted".into(),
                        name: "Wanted".into(),
                    },
                };
                if case == 0 {
                    client.verify_connection(input).await.map(|_| ())
                } else {
                    client
                        .onboard_connection(input, vec!["model".into()])
                        .await
                        .map(|_| ())
                }
            }
        });
        let frame = tokio::time::timeout(Duration::from_secs(1), reader.read())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let result = if case == 0 {
            json!({"kind":"verified","models":[{"id":"duplicate"},{"id":"duplicate"}]})
        } else {
            json!({"kind":"saved","connection":{"connectionId":"b746eb13-287c-4f3a-8590-dac93c0a1253",
                "revision":1,"slug":if case==1{"other"}else{"wanted"},"provider":provider(if case==1{"api"}else{"other"})}})
        };
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        assert!(matches!(
            request.await.unwrap(),
            Err(RequestFailure::Unknown(ClientError::Protocol(_)))
        ));
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn connection_mutations_bind_cas_and_acknowledgements_to_the_requested_identity() {
    use maka_protocol::{Operation, configuration::*};
    const ID: &str = "b746eb13-287c-4f3a-8590-dac93c0a1253";
    const OTHER: &str = "fe26c818-0e6a-47ce-861c-e8c28f053bbd";
    let expected = ConnectionVersionBasis {
        connection_id: ID.into(),
        revision: 7,
    };
    for (remove, result, valid) in [
        (
            false,
            json!({"kind":"committed","catalogRevision":12,"connection":{"connectionId":ID,"revision":8}}),
            true,
        ),
        (true, json!({"kind":"committed","catalogRevision":12}), true),
        (
            false,
            json!({"kind":"connection_stale","expected":expected,"actual":null}),
            true,
        ),
        (
            true,
            json!({"kind":"connection_stale","expected":expected,"actual":{"connectionId":ID,"revision":8}}),
            true,
        ),
        (
            false,
            json!({"kind":"committed","catalogRevision":12,"connection":{"connectionId":OTHER,"revision":8}}),
            false,
        ),
        (
            false,
            json!({"kind":"committed","catalogRevision":12,"connection":{"connectionId":ID,"revision":7}}),
            false,
        ),
        (
            true,
            json!({"kind":"connection_stale","expected":{"connectionId":OTHER,"revision":7},"actual":null}),
            false,
        ),
        (
            false,
            json!({"kind":"connection_stale","expected":expected,"actual":{"connectionId":OTHER,"revision":8}}),
            false,
        ),
        (
            true,
            json!({"kind":"connection_stale","expected":expected,"actual":expected}),
            false,
        ),
    ] {
        let operation = if remove {
            Operation::ConnectionCatalogRemove
        } else {
            Operation::ConnectionCatalogUpdate
        };
        decode_catalog_mutation_result(operation, &result).unwrap();
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            let expected = expected.clone();
            async move {
                if remove {
                    client
                        .remove_connection(RemoveCatalogConnectionInput { expected })
                        .await
                } else {
                    client
                        .update_connection(UpdateCatalogConnectionInput {
                            expected,
                            changes: ConnectionCatalogEntryUpdate {
                                name: "Connection".into(),
                                configuration: json!({}),
                                enabled: true,
                                enabled_model_ids: vec!["model".into()],
                                model_overrides: Patch::Keep,
                                request_body_overlay: Patch::Keep,
                            },
                        })
                        .await
                }
            }
        });
        let frame = tokio::time::timeout(Duration::from_secs(1), reader.read())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        if valid {
            assert_eq!(
                serde_json::to_value(request.await.unwrap().unwrap()).unwrap(),
                result
            );
            client.disconnect();
        } else {
            assert!(matches!(
                request.await.unwrap(),
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
        }
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
    }
}
