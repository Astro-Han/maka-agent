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
use maka_protocol::configuration::*;
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn credential_requests_bind_identity_cas_and_absence_without_exposing_saved_secrets() {
    let locator = CredentialLocator::Connection {
        connection_id: "b746eb13-287c-4f3a-8590-dac93c0a1253".into(),
        kind: ConnectionCredentialKind::RequestHeaders,
    };
    let id = "fe26c818-0e6a-47ce-861c-e8c28f053bbd";
    let basis = CredentialVersionBasis {
        locator: locator.clone(),
        credential_id: id.into(),
        revision: 7,
    };
    let status = |revision| json!({"locator":locator,"configured":true,"credentialId":id,"revision":revision,"updatedAt":100});
    let absent = json!({"locator":locator,"configured":false,"credentialId":null,"revision":null,"updatedAt":null});
    let mut other = status(8);
    other["locator"]["connectionId"] = json!("3f53c759-c5e6-4717-bad4-d59aab21e994");
    for (operation, result, valid) in [
        (0, json!({"kind":"status","status":status(7)}), true),
        (0, json!({"kind":"status","status":absent}), true),
        (0, json!({"kind":"status","status":other}), false),
        (
            1,
            json!({"kind":"committed","vaultRevision":12,"status":status(8)}),
            true,
        ),
        (
            1,
            json!({"kind":"committed","vaultRevision":12,"status":status(7)}),
            false,
        ),
        (
            1,
            json!({"kind":"committed","vaultRevision":12,"status":other}),
            false,
        ),
        (
            1,
            json!({"kind":"committed","vaultRevision":12,"status":absent}),
            false,
        ),
        (
            2,
            json!({"kind":"committed","vaultRevision":12,"status":absent}),
            true,
        ),
        (
            2,
            json!({"kind":"committed","vaultRevision":12,"status":status(8)}),
            false,
        ),
        (
            2,
            json!({"kind":"credential_stale","expected":basis,"actual":null}),
            true,
        ),
        (
            1,
            json!({"kind":"credential_stale","expected":basis,"actual":basis}),
            false,
        ),
    ] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            let locator = locator.clone();
            let basis = basis.clone();
            async move {
                if operation == 0 {
                    client
                        .credential_status(locator)
                        .await
                        .map(|v| serde_json::to_value(v).unwrap())
                } else if operation == 1 {
                    client
                        .set_credential(SetCredentialInput {
                            locator,
                            expected: Some(CredentialIdentityBasis {
                                credential_id: basis.credential_id,
                                revision: basis.revision,
                            }),
                            expected_connection: None,
                            secret: json!({"x-custom":"synthetic-new-key"}).to_string(),
                        })
                        .await
                        .map(|v| serde_json::to_value(v).unwrap())
                } else {
                    client
                        .delete_credential(DeleteCredentialInput { expected: basis })
                        .await
                        .map(|v| serde_json::to_value(v).unwrap())
                }
            }
        });
        let frame = tokio::time::timeout(Duration::from_secs(1), reader.read())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if operation == 1 {
            assert!(
                frame["input"].get("expectedConnection").is_none(),
                "absent optional basis is omitted"
            );
        }
        writer.write(&json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,"result":result})).await.unwrap();
        if valid {
            assert_eq!(request.await.unwrap().unwrap(), result);
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
