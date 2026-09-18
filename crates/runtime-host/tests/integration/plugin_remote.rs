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

use super::support::{client_probe::ClientFixture, peer::Peer};
use maka_plugins::{
    client::Bundle,
    kernel::Definition,
    package::{MANIFEST_FILE, Package},
};
use maka_runtime_host::{
    plugins::Setup,
    server::{Host, HostOptions, local::LocalListener},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

mod fixture;
mod javascript;
use fixture::{Example, State};

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn remote_fences_backend_replacement_and_owns_reads_pending_opens_and_documents() {
    tokio::time::timeout(Duration::from_secs(35), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-remote-plugin-");
    let package = Package::new(BTreeMap::from([
        (MANIFEST_FILE.into(), serde_json::to_vec(&json!({"schemaVersion":1,"id":"example.remote","client":{"entry":"client.js","sdkVersion":1}})).unwrap()),
        ("client.js".into(), b"immutable fixture".to_vec()),
    ])).unwrap();
    let bundle = Bundle::from_package(&package).unwrap().unwrap();
    let state = Arc::new(State::default());
    let mut setup = Setup::default();
    setup.builtins.insert(
        "example.remote".into(),
        Arc::new(Definition {
            id: "example.remote".into(),
            revision: package.digest().into(),
            dependencies: vec![],
            inject: vec![],
            plugin: Arc::new(Example {
                bundle,
                state: state.clone(),
            }),
        }),
    );
    setup.layers.insert("example.remote".into(), serde_json::from_value(json!([
        {"type":"insert","rootId":"profile","entry":{"id":"remote-host","packageId":"example.remote"}},
        {"type":"insert","rootId":"desktop-ui","entry":{"id":"remote-ui","packageId":"example.remote"}}
    ])).unwrap());
    let host = Host::open_with_options(
        fixture.owner(),
        None,
        HostOptions {
            plugins: setup,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("remote.sock");
    #[cfg(windows)]
    let endpoint =
        std::path::PathBuf::from(format!(r"\\.\pipe\maka-remote-{}", uuid::Uuid::new_v4()));
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop.clone()),
    );
    let mut peer = Peer::new(host.clone(), "remote-client").await;
    ready(&mut peer).await;
    let page = success(
        peer.rpc("plugin.client.query", json!({"kind":"snapshot"}))
            .await,
    );
    let entry = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["entryId"] == "remote-ui")
        .unwrap();
    let client = json!({"entryId":entry["entryId"],"extensionId":entry["extensionId"],"activation":entry["activation"],
        "contentDigest":entry["contentDigest"],"clientDigest":entry["clientDigest"]});
    let binding = json!({"client":client,"method":"echo","sessionId":null});
    let target = rpc(&mut peer, json!({"kind":"bind","binding":binding})).await["target"].clone();
    let document = rpc(&mut peer, json!({"kind":"open_document"})).await["document"].clone();
    let call = json!({"kind":"call","binding":binding,"target":target,"document":document,"input":{"hello":"world"}});
    assert_eq!(
        rpc(&mut peer, call.clone()).await["value"],
        json!({"input":{"hello":"world"},"client":"remote-client","session":null})
    );
    success(peer.rpc("plugin.composition.apply", json!({"operations":[{"type":"update","entryId":"remote-host","patch":{"config":{"changed":true}}}]})).await);
    ready(&mut peer).await;
    assert_eq!(
        peer.rpc("plugin.remote", call).await["error"]["code"],
        "operation_conflict"
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);

    let binding = json!({"client":client,"method":"events","sessionId":null});
    let target = rpc(&mut peer, json!({"kind":"bind","binding":binding})).await["target"].clone();
    let stream = rpc(
        &mut peer,
        json!({"kind":"open","binding":binding,"target":target,"document":document,"input":null}),
    )
    .await["stream"]
        .clone();
    let first = rpc(
        &mut peer,
        json!({"kind":"next","document":document,"stream":stream}),
    )
    .await;
    assert_eq!(first, json!({"kind":"item","item":null}));
    assert_eq!(
        rpc(
            &mut peer,
            json!({"kind":"next","document":document,"stream":stream})
        )
        .await["item"],
        "ready"
    );
    assert_eq!(
        rpc(
            &mut peer,
            json!({"kind":"next","document":document,"stream":stream}),
        )
        .await,
        json!({"kind":"pending"})
    );
    peer.send_rpc(
        "pending-read",
        "plugin.remote",
        json!({"kind":"next","document":document,"stream":stream}),
    );
    wait_count(&state.reads, 1).await;
    peer.send_rpc(
        "duplicate-read",
        "plugin.remote",
        json!({"kind":"next","document":document,"stream":stream}),
    );
    let duplicate = response(&mut peer).await;
    assert_eq!(duplicate["requestId"], "duplicate-read");
    assert_eq!(duplicate["error"]["code"], "invalid_request");
    assert_eq!(state.reads.load(Ordering::SeqCst), 1);
    let mut stranger = Peer::new(host.clone(), "another-client").await;
    assert_eq!(
        stranger
            .rpc(
                "plugin.remote",
                json!({"kind":"next","document":document,"stream":stream})
            )
            .await["error"]["code"],
        "operation_conflict"
    );
    stranger.close().await;
    peer.send_rpc(
        "close-document",
        "plugin.remote",
        json!({"kind":"close_document","document":document}),
    );
    let responses = [response(&mut peer).await, response(&mut peer).await];
    assert!(
        responses
            .iter()
            .any(|r| r["requestId"] == "close-document" && r["ok"] == true)
    );
    assert!(
        responses
            .iter()
            .any(|r| r["requestId"] == "pending-read" && r["ok"] == false)
    );
    assert_eq!(state.live.load(Ordering::SeqCst), 0);

    let document = rpc(&mut peer, json!({"kind":"open_document"})).await["document"].clone();
    for index in 0..32 {
        peer.send_rpc(&format!("late-{index}"), "plugin.remote",
            json!({"kind":"open","binding":binding,"target":target,"document":document,"input":"late"}));
    }
    wait_count(&state.opening, 32).await;
    peer.send_rpc(
        "excess",
        "plugin.remote",
        json!({"kind":"open","binding":binding,"target":target,"document":document,"input":"late"}),
    );
    let excess = response(&mut peer).await;
    assert_eq!(excess["requestId"], "excess");
    assert_eq!(excess["error"]["code"], "invalid_request");
    peer.send_rpc(
        "close-late",
        "plugin.remote",
        json!({"kind":"close_document","document":document}),
    );
    for _ in 0..33 {
        let reply = response(&mut peer).await;
        if reply["requestId"] == "close-late" {
            assert_eq!(reply["ok"], true, "{reply}");
        } else {
            assert_eq!(reply["error"]["code"], "operation_conflict", "{reply}");
        }
    }
    assert_eq!(state.opening.load(Ordering::SeqCst), 0);
    assert_eq!(state.live.load(Ordering::SeqCst), 0);
    // A connection close owns the same cleanup even without a document-close RPC.
    let document = rpc(&mut peer, json!({"kind":"open_document"})).await["document"].clone();
    rpc(
        &mut peer,
        json!({"kind":"open","binding":binding,"target":target,"document":document,"input":null}),
    )
    .await;
    peer.close().await;
    wait_count(&state.live, 0).await;
    stop.cancel();
    server.await.unwrap().unwrap();
    cleanup.disarm();
}
async fn wait_count(counter: &AtomicUsize, expected: usize) {
    while counter.load(Ordering::SeqCst) != expected {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
async fn response(peer: &mut Peer) -> Value {
    loop {
        let reply = peer.frame().await;
        if reply.get("requestId").is_some() {
            return reply;
        }
    }
}
fn success(reply: Value) -> Value {
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"].clone()
}
async fn rpc(peer: &mut Peer, input: Value) -> Value {
    success(peer.rpc("plugin.remote", input).await)
}
async fn ready(peer: &mut Peer) {
    loop {
        if success(
            peer.rpc("plugin.platform.query", json!({"view":"status"}))
                .await,
        )["convergence"]
            == "converged"
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
