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

use super::{ready, rpc, success};
use crate::support::{client_probe::ClientFixture, peer::Peer};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn javascript_remote_replaces_exact_registration_and_closes_late_vm_streams() {
    tokio::time::timeout(Duration::from_secs(30), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-js-remote-");
    let path = fixture.workspace.join("plugin");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(
        path.join("host.mjs"),
        include_str!("../../fixtures/remote-plugin.mjs"),
    )
    .unwrap();
    std::fs::write(path.join("client.js"), "immutable client fixture").unwrap();
    std::fs::write(
        path.join("maka.extension.json"),
        serde_json::to_vec(&json!({
            "schemaVersion":1,"id":"example.remote",
            "runtime":{"entry":"host.mjs","sdkVersion":1,"vm":"dedicated"},
            "client":{"entry":"client.js","sdkVersion":1},
        }))
        .unwrap(),
    )
    .unwrap();
    let host = Host::open(fixture.owner()).await.unwrap();
    #[cfg(unix)]
    let endpoint = fixture.workspace.parent().unwrap().join("js-remote.sock");
    #[cfg(windows)]
    let endpoint =
        std::path::PathBuf::from(format!(r"\\.\pipe\maka-js-remote-{}", uuid::Uuid::new_v4()));
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop.clone()),
    );
    let mut peer = Peer::new(host.clone(), "js-remote-client").await;
    success(
        peer.rpc("plugin.package.install", json!({"sourcePath":path}))
            .await,
    );
    success(peer.rpc("plugin.composition.apply",json!({"operations":[
        {"type":"insert","rootId":"profile","entry":{"id":"remote-host","packageId":"example.remote"}},
        {"type":"insert","rootId":"desktop-ui","entry":{"id":"remote-ui","packageId":"example.remote"}}
    ]})).await);
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
    let document = rpc(&mut peer, json!({"kind":"open_document"})).await["document"].clone();
    let (binding, target) = bind(&mut peer, &client, "echo").await;
    let call = json!({"kind":"call","binding":binding,"target":target,"document":document,"input":"hello"});
    assert_eq!(rpc(&mut peer, call.clone()).await["value"]["generation"], 0);
    let (replace, replacement) = bind(&mut peer, &client, "replace").await;
    rpc(&mut peer,json!({"kind":"call","binding":replace,"target":replacement,"document":document,"input":null})).await;
    assert_eq!(
        peer.rpc("plugin.remote", call).await["error"]["code"],
        "operation_conflict"
    );
    let (_, next) = bind(&mut peer, &client, "echo").await;
    assert_eq!(target["activation"], next["activation"]);
    assert_ne!(target["registration"], next["registration"]);
    assert_eq!(
        rpc(
            &mut peer,
            json!({"kind":"call","binding":binding,"target":next,"document":document,"input":null})
        )
        .await["value"]["generation"],
        1
    );
    let (binding, target) = bind(&mut peer, &client, "events").await;
    let stream = rpc(
        &mut peer,
        json!({"kind":"open","binding":binding,"target":target,"document":document,"input":null}),
    )
    .await["stream"]
        .clone();
    assert_eq!(
        rpc(
            &mut peer,
            json!({"kind":"next","document":document,"stream":stream})
        )
        .await,
        json!({"kind":"item","item":null})
    );
    peer.send_rpc(
        "read",
        "plugin.remote",
        json!({"kind":"next","document":document,"stream":stream}),
    );
    peer.send_rpc(
        "close",
        "plugin.remote",
        json!({"kind":"close","document":document,"stream":stream}),
    );
    let replies = [
        super::response(&mut peer).await,
        super::response(&mut peer).await,
    ];
    assert!(
        replies
            .iter()
            .any(|reply| reply["requestId"] == "close" && reply["ok"] == true),
        "{replies:?}"
    );
    assert!(
        replies.iter().any(|reply| reply["requestId"] == "read"
            && (reply["error"]["code"] == "operation_conflict"
                || reply["result"]["kind"] == "end")),
        "{replies:?}"
    );
    let (stats, stats_target) = bind(&mut peer, &client, "stats").await;
    let status = rpc(&mut peer,json!({"kind":"call","binding":stats,"target":stats_target,"document":document,"input":null})).await;
    assert_eq!(status["value"]["active"], 0);
    assert_eq!(status["value"]["stopped"], 1);

    peer.send_rpc(
        "late",
        "plugin.remote",
        json!({"kind":"open","binding":binding,"target":target,"document":document,"input":"late"}),
    );
    loop {
        let status = rpc(&mut peer,json!({"kind":"call","binding":stats,"target":stats_target,"document":document,"input":null})).await;
        if status["value"]["opening"] == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    peer.send_rpc(
        "end-page",
        "plugin.remote",
        json!({"kind":"close_document","document":document}),
    );
    let responses = [
        super::response(&mut peer).await,
        super::response(&mut peer).await,
    ];
    assert!(
        responses
            .iter()
            .any(|r| r["requestId"] == "end-page" && r["ok"] == true),
        "{responses:?}"
    );
    assert!(
        responses
            .iter()
            .any(|r| r["requestId"] == "late" && r["error"]["code"] == "operation_conflict"),
        "{responses:?}"
    );
    let document = rpc(&mut peer, json!({"kind":"open_document"})).await["document"].clone();
    let status = rpc(&mut peer,json!({"kind":"call","binding":stats,"target":stats_target,"document":document,"input":null})).await;
    assert_eq!(
        status["value"],
        json!({"generation":1,"opening":0,"active":0,"stopped":2})
    );
    let client = tokio::process::Command::new("node")
        .kill_on_drop(true)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/client.mjs"),
        )
        .arg("--socket")
        .arg(&endpoint)
        .args(["--root-id", host.root_id(), "--plugin-remote"])
        .output()
        .await
        .unwrap();
    assert!(
        client.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&client.stdout),
        String::from_utf8_lossy(&client.stderr)
    );
    assert!(String::from_utf8_lossy(&client.stdout).contains("original-client-plugin-remote"));
    peer.close().await;
    stop.cancel();
    server.await.unwrap().unwrap();
    cleanup.disarm();
}
async fn bind(peer: &mut Peer, client: &Value, method: &str) -> (Value, Value) {
    let binding = json!({"client":client,"method":method,"sessionId":null});
    let target = rpc(peer, json!({"kind":"bind","binding":binding})).await["target"].clone();
    (binding, target)
}
