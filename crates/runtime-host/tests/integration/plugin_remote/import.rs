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
use super::{rpc, success};
use maka_runtime_host::session::SessionModel;
use serde_json::{Value, json};
use std::path::Path;

pub(super) async fn verify(
    peer: &mut Peer,
    fixture: &ClientFixture,
    model: &SessionModel,
    database: &Path,
    client: &Value,
    document: &Value,
) {
    let binding = json!({"client":client,"method":"imports","sessionId":null});
    let target = rpc(peer, json!({"kind":"bind","binding":binding})).await["target"].clone();
    let mut call = json!({"kind":"call","binding":binding,"target":target,"document":document});
    let source = uuid::Uuid::new_v4();
    call["input"] = json!({"kind":"save_sources","expectedRevision":null,"configuration":{"sources":[{
        "id":source,"name":"OpenCode","location":{"kind":"open_code","database":database}
    }]}});
    let sources = rpc(peer, call.clone()).await["value"].clone();
    let revision = sources["snapshot"]["revision"].as_u64().unwrap();
    call["input"] =
        json!({"kind":"catalog","sourceId":source,"revision":revision,"query":{"limit":20}});
    let catalog = rpc(peer, call.clone()).await["value"].clone();
    assert!(
        catalog["page"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "selected")
    );
    let destination = fixture.workspace.join("import-destination");
    std::fs::create_dir(&destination).unwrap();
    let id = uuid::Uuid::new_v4();
    let request = json!({
        "operationId":id,
        "selection":{"sourceId":source,"sourceRevision":revision,"sessionId":"selected","path":database},
        "workspace":{"kind":"host_path","path":destination},
        "settings":{
            "target":{"kind":"model","model":model,"thinkingLevel":null},
            "sandboxMode":"read-only","approvalPolicy":maka_runtime::execution::ApprovalPolicy::OnRequest,"collaborationMode":"agent",
            "behavior":"default"
        }
    });
    call["input"] = json!({"kind":"prepare","request":request});
    let prepared = rpc(peer, call.clone()).await["value"].clone();
    assert_eq!(prepared["kind"], "copy", "{prepared}");
    // Stable identity cannot be repurposed for the source workspace.
    call["input"]["request"]["workspace"]["path"] = json!(fixture.workspace);
    assert_eq!(rpc(peer, call.clone()).await["value"]["kind"], "conflict");
    // Source reconfiguration invalidates old selections, not existing intents.
    call["input"] =
        json!({"kind":"save_sources","expectedRevision":revision,"configuration":{"sources":[]}});
    rpc(peer, call.clone()).await;
    call["input"] = json!({"kind":"prepare","request":request});
    assert_eq!(rpc(peer, call.clone()).await["value"], prepared);
    let mut new_request = request.clone();
    new_request["operationId"] = json!(uuid::Uuid::new_v4());
    call["input"] = json!({"kind":"prepare","request":new_request});
    assert_eq!(rpc(peer, call.clone()).await["value"]["kind"], "conflict");
    call["input"] = json!({"kind":"deliver","operationId":id});
    let delivered = rpc(peer, call.clone()).await["value"].clone();
    let receipt = &delivered["copy"]["receipt"];
    assert_eq!(receipt["state"], "published", "{delivered}");
    assert_eq!(receipt["records"], 1);
    // Lost reply: inspect the canonical receipt, without rereading the source.
    assert_eq!(rpc(peer, call.clone()).await["value"], delivered);
    let session = success(
        peer.rpc(
            "session.catalog.query",
            json!({"kind":"get","sessionId":receipt["sessionId"]}),
        )
        .await,
    )["session"]
        .clone();
    assert_eq!(
        Path::new(session["workspace"]["hostCwd"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        destination.canonicalize().unwrap(),
    );
    assert_eq!(session["name"], prepared["copy"]["title"]);
    call["input"] = json!({"kind":"abandon","operationId":id});
    assert_eq!(rpc(peer, call.clone()).await["value"], delivered);
    call["input"] = json!({"kind":"copies","after":null});
    let copies = rpc(peer, call.clone()).await["value"].clone();
    assert_eq!(
        copies["page"]["copies"].as_array().unwrap(),
        &vec![delivered["copy"].clone()]
    );
    // Client-supplied authority fields are rejected, never used as a destination.
    call["input"] = json!({"kind":"deliver","operationId":id,"workspace":{"kind":"host_path","path":fixture.workspace}});
    let rejected = peer.rpc("plugin.remote", call).await;
    assert_ne!(rejected["ok"], true, "{rejected}");
}
