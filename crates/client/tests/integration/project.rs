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
use maka_protocol::project::{Mutation, Query, View};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn project_catalog_checks_variant_revision_directory_identity_and_progress() {
    let revision = format!("sha256:{}", "a".repeat(64));
    let other = format!("sha256:{}", "b".repeat(64));
    let start = Query::ListStart {
        view: View::Summary,
    };
    let next = Query::ListContinue {
        view: View::Summary,
        revision: revision.clone(),
        cursor: "64".into(),
    };
    let item = json!({"kind":"project","projectIndex":0,"id":"p","name":"Project",
        "aliasCount":0,"locationCount":1,"preferredLocationIndex":0,"archivedAt":null,"available":true});
    for (input, result) in [
        (start.clone(), json!({"kind":"directory_roots","roots":[]})),
        (
            Query::DirectoryResolve {
                root_id: "root".into(),
                segments: vec!["chosen".into()],
            },
            json!({"kind":"directory_path","rootId":"other","segments":["chosen"],"path":"/resolved"}),
        ),
        (
            Query::DirectoryResolve {
                root_id: "root".into(),
                segments: vec!["chosen".into()],
            },
            json!({"kind":"directory_path","rootId":"root","segments":["other"],"path":"/resolved"}),
        ),
        (
            Query::DirectoryResolve {
                root_id: "root".into(),
                segments: vec![],
            },
            json!({"kind":"directory_page","rootId":"root","segments":[],"entries":[],"nextCursor":null}),
        ),
        (
            start.clone(),
            json!({"kind":"revision_changed","view":"summary","expected":revision,"actual":other}),
        ),
        (
            start.clone(),
            json!({"kind":"page","view":"locations","revision":revision,"projectCount":1,"items":[item],"nextCursor":null}),
        ),
        (
            next.clone(),
            json!({"kind":"page","view":"summary","revision":other,"projectCount":1,"items":[item],"nextCursor":null}),
        ),
        (
            next,
            json!({"kind":"page","view":"summary","revision":revision,"projectCount":1,"items":[item],"nextCursor":"64"}),
        ),
        (
            start.clone(),
            json!({"kind":"page","view":"summary","revision":revision,"projectCount":1,"items":[item,item],"nextCursor":null}),
        ),
        (
            start,
            json!({"kind":"page","view":"summary","revision":revision,"projectCount":1,"items":[],"nextCursor":"64"}),
        ),
        (
            Query::DirectoryListStart {
                root_id: "root".into(),
                segments: vec!["wanted".into()],
            },
            json!({"kind":"directory_page","rootId":"root","segments":["wrong"],"entries":[],"nextCursor":null}),
        ),
        (
            Query::DirectoryRoots,
            json!({"kind":"directory_roots","roots":[{"id":"r","label":"A"},{"id":"r","label":"B"}]}),
        ),
        (
            Query::DirectoryListStart {
                root_id: "root".into(),
                segments: vec![],
            },
            json!({"kind":"directory_page","rootId":"root","segments":[],"entries":[],"nextCursor":"again"}),
        ),
        (
            Query::DirectoryListStart {
                root_id: "root".into(),
                segments: vec![],
            },
            json!({"kind":"directory_page","rootId":"root","segments":[],"entries":[{"name":"a"},{"name":"a"}],"nextCursor":null}),
        ),
        (
            Query::DirectoryListStart {
                root_id: "root".into(),
                segments: vec![],
            },
            json!({"kind":"directory_page","rootId":"root","segments":[],"entries":[{"name":"a"}],"nextCursor":"z"}),
        ),
        (
            Query::DirectoryListContinue {
                root_id: "root".into(),
                segments: vec![],
                cursor: "z".into(),
            },
            json!({"kind":"directory_page","rootId":"root","segments":[],"entries":[{"name":"a"}],"nextCursor":null}),
        ),
    ] {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let request = tokio::spawn({
            let client = client.clone();
            async move { client.project_catalog(input).await }
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
async fn project_mutations_check_identity_and_lifecycle_but_accept_canonical_aliases() {
    for case in 0..4 {
        let (client, _notices, mut reader, mut writer) = pair_with(maka_client::Operations).await;
        let input = match case {
            1 => Mutation::Archive {
                project_id: "wanted".into(),
            },
            2 => Mutation::Restore {
                project_id: "wanted".into(),
            },
            _ => Mutation::Rename {
                project_id: "wanted".into(),
                name: "name".into(),
            },
        };
        let request = tokio::spawn({
            let client = client.clone();
            async move { client.mutate_project(input).await }
        });
        let frame = reader.read().await.unwrap().unwrap();
        let project = json!({"id":if case == 0 || case == 3 {"canonical"} else {"wanted"},
            "aliases":if case == 3 {vec!["wanted"]} else {vec![]}, "name":"name",
            "locationCount":1,"archivedAt":if case == 2 {Some(1)} else {None},
            "available":true});
        writer
            .write(
                &json!({"requestId":frame["requestId"],"operation":frame["operation"],"ok":true,
            "result":{"kind":"project","project":project}}),
            )
            .await
            .unwrap();
        if case == 3 {
            assert_eq!(request.await.unwrap().unwrap().id, "canonical");
            client.disconnect();
        } else {
            assert!(matches!(
                request.await.unwrap(),
                Err(RequestFailure::Unknown(ClientError::Protocol(_)))
            ));
            tokio::time::timeout(Duration::from_secs(1), client.closed())
                .await
                .unwrap();
        }
    }
}
