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

use super::super::support::{client_probe::ClientFixture, peer::Peer};
use maka_client::{Client, ClientError, RequestFailure};
use maka_protocol::{
    OperationErrorCode,
    project::{Query, QueryResult},
};
use maka_runtime_host::server::{DirectoryRootSpec, Host, HostOptions, local::LocalListener};
use std::{os::unix::fs::symlink, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolves_selected_directory_without_registration_or_root_escape() {
    let fixture = ClientFixture::new("maka-directory-reference-");
    let nested = fixture.workspace.join("实际目录");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(fixture.workspace.join("not-directory"), b"file").unwrap();
    let outside = fixture.workspace.parent().unwrap().join("outside");
    std::fs::create_dir(&outside).unwrap();
    symlink(&nested, fixture.workspace.join("显示别名")).unwrap();
    symlink(&outside, fixture.workspace.join("escape")).unwrap();
    let owner = fixture.owner();
    let root_id = owner.root_id().to_owned();
    let host = Host::open_with_options(
        owner,
        None,
        HostOptions {
            project_directory_roots: Some(vec![DirectoryRootSpec {
                label: "Published label".into(),
                path: fixture.workspace.clone(),
            }]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (peer, hello) = Peer::handshake(host.clone(), "directory-reference-bootstrap").await;
    peer.close().await;
    let socket = fixture.workspace.parent().unwrap().join("host.sock");
    let listener = LocalListener::bind(&socket).unwrap();
    let stop = CancellationToken::new();
    let _cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(listener.serve(host, stop.clone()));
    let (client, _notices) = Client::connect(
        maka_client::local::open_stream(&socket).await.unwrap(),
        &root_id,
        hello["hostEpoch"].as_str().unwrap(),
        maka_client::Operations,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        let QueryResult::DirectoryRoots {roots} = client.project_catalog(Query::DirectoryRoots).await.unwrap() else {panic!()};
        assert_eq!(roots.len(),1);
        let id = roots[0].id.clone();
        assert_eq!(roots[0].label,"Published label");
        let QueryResult::DirectoryPage {entries,..} = client.project_catalog(Query::DirectoryListStart {root_id:id.clone(),segments:vec![]}).await.unwrap() else {panic!()};
        assert!(entries.iter().any(|entry|entry.name=="显示别名"));
        assert!(!entries.iter().any(|entry|entry.name=="escape"));
        for (segments,expected) in [(vec![],fixture.workspace.clone()),(vec!["显示别名".into()],nested.clone())] {
            let QueryResult::DirectoryPath {root_id,segments:selected,path} = client.project_catalog(Query::DirectoryResolve {root_id:id.clone(),segments:segments.clone()}).await.unwrap() else {panic!()};
            assert_eq!(root_id,id);
            assert_eq!(selected,segments);
            assert_eq!(path,expected.canonicalize().unwrap().to_str().unwrap());
        }
        for (root_id,segments) in [
            ("unknown",vec![]),(id.as_str(),vec!["escape".into()]),
            (id.as_str(),vec!["not-directory".into()]),(id.as_str(),vec!["missing".into()])
        ] {
            let result = client.project_catalog(Query::DirectoryResolve {root_id:root_id.into(),segments}).await;
            assert!(matches!(result,Err(RequestFailure::Rejected(ClientError::Rejected(error))) if error.code==OperationErrorCode::InvalidRequest));
        }
        std::fs::rename(&fixture.workspace,fixture.workspace.with_file_name("replaced-root")).unwrap();
        std::fs::create_dir(&fixture.workspace).unwrap();
        assert!(matches!(client.project_catalog(Query::DirectoryResolve {root_id:id,segments:vec![]}).await,
            Err(RequestFailure::Rejected(ClientError::Rejected(error))) if error.code==OperationErrorCode::InvalidRequest));
    }).await.unwrap();
    client.disconnect();
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let log = fixture.log().await;
    assert!(
        log.list_projects().await.unwrap().is_empty(),
        "lookup must not register a project"
    );
    assert!(log.prefix(1, 4096).await.unwrap().events.is_empty());
    log.close().await.unwrap();
}
