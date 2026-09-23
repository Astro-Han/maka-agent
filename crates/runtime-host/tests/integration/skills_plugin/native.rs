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

use super::{ClientFixture, Host, LocalListener, Peer, Provider, configure, converged, disabled};
use maka_client::{Client, ClientError, Operations, RequestFailure};
use maka_protocol::{
    OperationErrorCode,
    plugin::{RemoteBinding, RemoteKind, RemoteRequest, RemoteResult},
};
use maka_skills::api::{InvocableResult, MAX_ITEMS};
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_candidates_page_by_session_and_reject_changed_catalog_and_retired_binding() {
    let fixture = ClientFixture::new("maka-native-candidates-");
    for index in 0..=MAX_ITEMS {
        let dir = fixture
            .workspace
            .join(format!(".maka/skills/review-{index:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!(
            "---\nname: Review {index}\ndescription: Review code carefully\n---\nReview with care.\n")).unwrap();
    }
    let provider = Provider::start().await;
    let model = configure(&fixture, &provider.base_url).await;
    let host = Host::open(fixture.owner()).await.unwrap();
    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("candidates.sock");
    #[cfg(windows)]
    let endpoint = std::path::PathBuf::from(format!(
        r"\\.\pipe\maka-candidates-{}",
        uuid::Uuid::new_v4()
    ));
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop),
    );
    let (mut peer, hello) = Peer::handshake(host.clone(), "candidates-fixture").await;
    converged(&mut peer).await;
    assert_eq!(
        peer.rpc(
            "session.create",
            json!({
                "sessionId":"candidates","workspace":{"kind":"host_path","path":fixture.workspace},
                "modelTarget":{"kind":"explicit","connectionId":model.connection_id,
                    "connectionSlug":model.connection_slug,"model":model.model}
            })
        )
        .await["ok"],
        true
    );
    let (client, _notices) = Client::connect(
        maka_client::local::open_stream(&endpoint).await.unwrap(),
        host.root_id(),
        hello["hostEpoch"].as_str().unwrap(),
        Operations,
    )
    .await
    .unwrap();
    // A terminal uses the backend package, not a frontend bundle or Desktop entry.
    let binding = RemoteBinding::Package {
        package_id: "maka.skills".into(),
        method: "request".into(),
        session_id: Some("candidates".into()),
    };
    let RemoteResult::Bound {
        target,
        handler: RemoteKind::Method,
    } = client
        .plugin_remote(RemoteRequest::Bind {
            binding: binding.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("method expected")
    };
    let RemoteResult::Document { document } = client
        .plugin_remote(RemoteRequest::OpenDocument)
        .await
        .unwrap()
    else {
        panic!("document expected")
    };
    let call = |page| RemoteRequest::Call {
        binding: binding.clone(),
        target: target.clone(),
        document,
        input: json!({"kind":"invocable","page":page}),
    };
    let decode = |result| {
        let RemoteResult::Value { value } = result else {
            panic!("value expected")
        };
        serde_json::from_value::<InvocableResult>(value).unwrap()
    };
    let InvocableResult::Page {
        revision,
        items,
        next_cursor: Some(cursor),
    } = decode(
        client
            .plugin_remote(call(serde_json::Value::Null))
            .await
            .unwrap(),
    )
    else {
        panic!("first page must be bounded")
    };
    assert_eq!(items.len(), MAX_ITEMS);
    let mut ids: BTreeSet<_> = items.into_iter().map(|item| item.id).collect();
    let page = json!({"revision":revision,"cursor":cursor});
    let InvocableResult::Page {
        revision: next_revision,
        items,
        next_cursor: None,
    } = decode(client.plugin_remote(call(page.clone())).await.unwrap())
    else {
        panic!("last page expected")
    };
    assert_eq!(next_revision, revision);
    assert_eq!(items.len(), 1);
    assert!(ids.insert(items[0].id.clone()));
    assert_eq!(ids.len(), MAX_ITEMS + 1);
    // A real file change invalidates a cursor; no automatic restart hides the change.
    std::fs::write(
        fixture.workspace.join(".maka/skills/review-000/SKILL.md"),
        "---\nname: Changed\ndescription: Updated\n---\nChanged instructions.",
    )
    .unwrap();
    let InvocableResult::RevisionChanged {
        expected_revision,
        actual_revision,
    } = decode(client.plugin_remote(call(page)).await.unwrap())
    else {
        panic!("revision changed")
    };
    assert_eq!(expected_revision, revision);
    assert_ne!(actual_revision, revision);
    // Even a later reactivation cannot make the captured target refer to replacement code.
    disabled(&mut peer, true).await;
    disabled(&mut peer, false).await;
    assert!(
        matches!(client.plugin_remote(call(serde_json::Value::Null)).await,
        Err(RequestFailure::Rejected(ClientError::Rejected(error)))
            if error.code == OperationErrorCode::OperationConflict)
    );
    assert!(matches!(
        client
            .plugin_remote(RemoteRequest::CloseDocument { document })
            .await
            .unwrap(),
        RemoteResult::Closed
    ));
    assert!(
        provider.requests.lock().unwrap().is_empty(),
        "browsing must not start a model"
    );
    client.disconnect();
    peer.close().await;
    drop(cleanup);
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
