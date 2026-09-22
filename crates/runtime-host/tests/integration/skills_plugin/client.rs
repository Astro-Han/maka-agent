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

use super::{Peer, converged, disabled, json};

pub(crate) async fn authorization(
    peer: &mut Peer,
    command: serde_json::Value,
) -> serde_json::Value {
    let snapshot = peer
        .rpc("plugin.client.query", json!({"kind":"snapshot"}))
        .await;
    let entry = snapshot["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == "maka.skills")
        .unwrap();
    let client = json!({"entryId":entry["entryId"],"extensionId":entry["extensionId"],
        "activation":entry["activation"],"contentDigest":entry["contentDigest"],"clientDigest":entry["clientDigest"]});
    let result = peer
        .rpc(
            "plugin.authorization",
            json!({"client":client,"scope":"profile","command":command}),
        )
        .await;
    assert_eq!(result["ok"], true, "{result}");
    result["result"].clone()
}

pub(crate) async fn request(
    peer: &mut Peer,
    method: &str,
    input: serde_json::Value,
) -> serde_json::Value {
    let snapshot = peer
        .rpc("plugin.client.query", json!({"kind":"snapshot"}))
        .await;
    let entry = snapshot["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == "maka.skills")
        .unwrap();
    let client = json!({"entryId":entry["entryId"],"extensionId":entry["extensionId"],
        "activation":entry["activation"],"contentDigest":entry["contentDigest"],"clientDigest":entry["clientDigest"]});
    let binding = json!({"client":client,"method":method});
    let bound = peer
        .rpc("plugin.remote", json!({"kind":"bind","binding":binding}))
        .await;
    assert_eq!(bound["ok"], true, "{bound}");
    let opened = peer
        .rpc("plugin.remote", json!({"kind":"open_document"}))
        .await;
    assert_eq!(opened["ok"], true, "{opened}");
    let document = &opened["result"]["document"];
    let outcome = peer
        .rpc(
            "plugin.remote",
            json!({"kind":"call","document":document,
        "binding":binding,"target":bound["result"]["target"],"input":input}),
        )
        .await;
    let closed = peer
        .rpc(
            "plugin.remote",
            json!({"kind":"close_document","document":document}),
        )
        .await;
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(outcome["ok"], true, "{outcome}");
    outcome["result"]["value"].clone()
}

pub(crate) async fn workspace(
    peer: &mut Peer,
    path: &serde_json::Value,
    input: serde_json::Value,
) -> serde_json::Value {
    request(
        peer,
        "path-request",
        json!({"path":path,"sandboxMode":"workspace-write",
        "collaborationMode":"agent","request":input}),
    )
    .await
}

pub(super) async fn verify(peer: &mut Peer) {
    let snapshot = peer
        .rpc("plugin.client.query", json!({"kind":"snapshot"}))
        .await;
    assert_eq!(snapshot["ok"], true, "{snapshot}");
    let entry = snapshot["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == "maka.skills")
        .unwrap();
    let client = json!({"entryId":entry["entryId"],"extensionId":entry["extensionId"],
        "activation":entry["activation"],"contentDigest":entry["contentDigest"],"clientDigest":entry["clientDigest"]});
    let binding = json!({"client":client,"method":"request","sessionId":"skills-session"});
    let bound = peer
        .rpc("plugin.remote", json!({"kind":"bind","binding":binding}))
        .await;
    assert_eq!(bound["ok"], true, "{bound}");
    let document = peer
        .rpc("plugin.remote", json!({"kind":"open_document"}))
        .await["result"]["document"]
        .clone();
    let mut call = json!({"kind":"call","document":document,"binding":binding,"target":bound["result"]["target"],
        "input":{"kind":"invocable","page":null}});
    let available = peer.rpc("plugin.remote", call.clone()).await;
    assert_eq!(available["ok"], true, "{available}");
    assert!(
        available["result"]["value"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "review")
    );
    call["input"] = json!({"kind":"catalog","view":"governance","page":null,"context":{"workspace":{"kind":"host_path","path":"/not-authorized"}}});
    let denied = peer.rpc("plugin.remote", call.clone()).await;
    assert_eq!(
        denied["ok"], false,
        "Remote cannot smuggle workspace authority: {denied}"
    );
    call["input"] = json!({"kind":"catalog","view":"governance","page":null});
    let catalog = peer.rpc("plugin.remote", call.clone()).await;
    assert_eq!(catalog["ok"], true, "{catalog}");
    let revision = &catalog["result"]["value"]["revision"];
    call["input"] = json!({"kind":"mutate","expectedRevision":revision,
        "mutation":{"kind":"set_pinned","ref":"project:maka:review","pinned":true}});
    let pinned = peer.rpc("plugin.remote", call.clone()).await;
    assert_eq!(pinned["result"]["value"]["kind"], "committed", "{pinned}");
    assert_eq!(pinned["result"]["value"]["entry"]["pinned"], true);
    let stale = peer.rpc("plugin.remote", call.clone()).await;
    assert_eq!(
        stale["result"]["value"]["kind"], "revision_conflict",
        "{stale}"
    );
    let disabled_backend = peer
        .rpc(
            "plugin.composition.apply",
            json!({
                "operations":[{"type":"update","entryId":"maka.skills","patch":{"disabled":true}}]
            }),
        )
        .await;
    assert_eq!(disabled_backend["ok"], true, "{disabled_backend}");
    // The UI withdraws through its actual service dependency, without a UI patch.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let snapshot = peer
                .rpc("plugin.client.query", json!({"kind":"snapshot"}))
                .await;
            if !snapshot["result"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["extensionId"] == "maka.skills")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let retired = peer.rpc("plugin.remote", call).await;
    assert_eq!(retired["error"]["code"], "operation_conflict", "{retired}");
    let closed = peer
        .rpc(
            "plugin.remote",
            json!({"kind":"close_document","document":document}),
        )
        .await;
    assert_eq!(closed["ok"], true, "{closed}");
    disabled(peer, true).await;
    converged(peer).await;
}
