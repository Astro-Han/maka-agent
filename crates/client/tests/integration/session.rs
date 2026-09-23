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
use maka_protocol::session::*;
use serde_json::json;
use std::time::Duration;

mod copy;

#[tokio::test]
async fn catalog_rejects_wrong_identity_revision_variant_and_nonadvancing_pages() {
    let revision = format!("sha256:{}", "a".repeat(64));
    let other = format!("sha256:{}", "b".repeat(64));
    let item = json!({
        "id":"wrong","revision":1,"workspace":{"target":{"kind":"host_path","path":"/work"},"hostCwd":"/work"},
        "createdAt":0,"activityAt":1,"name":"Chat","isFlagged":false,"isArchived":false,
        "labels":[],"labelsTruncated":false,"hasUnread":false,"status":"active","backend":"ai-sdk",
        "llmConnectionId":null,"llmConnectionSlug":"default","connectionLocked":false,"model":"model",
        "sandboxMode":"workspace-write","approvalPolicy":{"kind":"on-request"},"collaborationMode":"agent","orchestrationMode":"default"
    });
    let continuation = SessionCatalogQueryInput::ListContinue {
        revision: revision.clone(),
        cursor: "cursor".into(),
    };
    for (input, result) in [
        (
            SessionCatalogQueryInput::PendingStart,
            json!({"kind":"page","revision":revision,"sessions":[item],"nextCursor":null}),
        ),
        (
            SessionCatalogQueryInput::Get {
                session_id: "wanted".into(),
            },
            json!({"kind":"session","session":item}),
        ),
        (
            continuation.clone(),
            json!({"kind":"page","revision":other,"sessions":[item],"nextCursor":null}),
        ),
        (
            continuation,
            json!({"kind":"page","revision":revision,"sessions":[item],"nextCursor":"cursor"}),
        ),
        (
            SessionCatalogQueryInput::ListStart,
            json!({"kind":"session","session":null}),
        ),
        (
            SessionCatalogQueryInput::ListStart,
            json!({"kind":"page","revision":revision,"sessions":[item,item],"nextCursor":null}),
        ),
        (
            SessionCatalogQueryInput::ListStart,
            json!({"kind":"page","revision":revision,"sessions":[],"nextCursor":"cursor"}),
        ),
    ] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move { client.session_catalog(input).await }
        });
        let frame = reader.read().await.unwrap().unwrap();
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
async fn mutations_reject_foreign_identity_revision_and_lifecycle_acknowledgements() {
    for case in 0..8 {
        let item = json!({
            "id":if case == 3 {"wanted"} else {"wrong"},"revision":2,"workspace":{"target":{"kind":"host_path","path":"/work"},"hostCwd":"/work"},
            "createdAt":0,"activityAt":1,"name":"Chat","isFlagged":false,"isArchived":case != 3,
            "labels":[],"labelsTruncated":false,"hasUnread":false,"status":"active","backend":"ai-sdk",
            "llmConnectionId":null,"llmConnectionSlug":"default","connectionLocked":false,"model":"model",
            "sandboxMode":"workspace-write","approvalPolicy":{"kind":"on-request"},"collaborationMode":"agent","orchestrationMode":"default"
        });
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move {
                if case < 2 {
                    client
                        .update_session_metadata(SessionMetadataUpdateInput {
                            session_id: "wanted".into(),
                            expected_revision: 1,
                            patch: SessionMetadataPatch {
                                name: Some("Chat".into()),
                                labels: None,
                                is_flagged: None,
                            },
                        })
                        .await
                        .map(|_| ())
                } else if case < 4 {
                    client
                        .set_session_lifecycle(SessionLifecycleSetInput {
                            session_id: "wanted".into(),
                            state: SessionLifecycleState::Archived,
                        })
                        .await
                        .map(|_| ())
                } else if case < 6 {
                    client
                        .relocate_session_workspace(SessionWorkspaceRelocateInput {
                            session_id: "wanted".into(),
                            expected_revision: 1,
                            workspace: WorkspaceTarget::HostPath {
                                path: "/work".into(),
                            },
                        })
                        .await
                        .map(|_| ())
                } else {
                    client.update_session_configuration(decode_session_configuration_update_input(&json!({
                        "sessionId":"wanted","expectedRevision":1,"patch":{"modelTarget":{"kind":"explicit","connectionId":"connection","connectionSlug":"default","model":"model"},"thinkingLevel":null}
                    })).unwrap()).await.map(|_| ())
                }
            }
        });
        let frame = reader.read().await.unwrap().unwrap();
        let result = match case {
            0 | 4 | 6 => json!({"kind":"committed", "session":item}),
            1 | 5 | 7 => {
                json!({"kind":"revision_conflict", "expectedRevision":8, "actualRevision":9})
            }
            _ => item,
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
async fn removal_rejects_foreign_receipts_and_never_retries_an_unknown_mutation() {
    for case in 0..4 {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move {
                if case == 2 {
                    client
                        .query_session_removal(SessionRemoveQueryInput {
                            session_id: "wanted".into(),
                        })
                        .await
                        .map(|_| ())
                } else {
                    client
                        .remove_session(SessionRemoveInput {
                            session_id: "wanted".into(),
                            expected_revision: 7,
                        })
                        .await
                        .map(|_| ())
                }
            }
        });
        let frame = reader.read().await.unwrap().unwrap();
        assert_eq!(frame["input"]["sessionId"], "wanted");
        if case == 3 {
            // The Host may have accepted the write; transport loss must not retry it.
            writer.close_after_flush().await.unwrap();
            assert!(matches!(
                request.await.unwrap(),
                Err(RequestFailure::Unknown(_))
            ));
        } else {
            let result = if case == 1 {
                json!({"kind":"revision_conflict","expectedRevision":8,"actualRevision":9})
            } else {
                json!({"kind":"removed","sessionId":"other","archivedSubtaskCount":0})
            };
            writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
            assert!(matches!(
                request.await.unwrap(),
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
        }
        tokio::time::timeout(Duration::from_secs(1), client.closed())
            .await
            .unwrap();
        assert!(
            reader.read().await.unwrap().is_none(),
            "must not resend removal"
        );
    }
}
