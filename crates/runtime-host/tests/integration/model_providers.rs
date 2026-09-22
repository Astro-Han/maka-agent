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

use super::{
    javascript_plugins::{package, ready},
    support::{client_probe::ClientFixture, peer::Peer},
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const PROVIDER: &str = r#"
export default async function(ctx) {
    await ctx.modelProviders.register('example.account', {
        label: 'Example Account',
        configurationSchema: {type:'object',properties:{baseUrl:{type:'string'}},required:['baseUrl'],additionalProperties:false},
        configurationDefaults: {baseUrl:'https://provider.invalid/v1'},
        authentication: [{id:'key',label:'API key',inputSchema:{type:'object',properties:{key:{type:'string'}},required:['key']},interactive:false}],
        discovery: false,
    }, {
        resolve: ({model,connection}) => ({
            adapter:'sdk',protocol:'openai_chat',baseUrl:connection.configuration.baseUrl,
            info:model,thinkingLevels:[],providerOptions:{},
        }),
        authorize: ({credential}) => ({apiKey:credential.secret}),
        authenticate: ({input}) => ({secret:input.key,refreshAt:null}),
    });
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn provider_directory_uses_published_identity_and_invalidates_cursors_on_retirement() {
    tokio::time::timeout(Duration::from_secs(60), async {
        for mode in ["shared", "dedicated"] {
            let fixture = ClientFixture::new("maka-provider-directory-");
            let source = package(&fixture.workspace, "example.account", mode, PROVIDER, false);
            let host = Host::open(fixture.owner()).await.unwrap();
            #[cfg(unix)]
            let endpoint = fixture.workspace.parent().unwrap().join("providers.sock");
            #[cfg(windows)]
            let endpoint = std::path::PathBuf::from(format!(r"\\.\pipe\maka-providers-{}", uuid::Uuid::new_v4()));
            let stop = CancellationToken::new();
            let _cleanup = stop.clone().drop_guard();
            let server = tokio::spawn(LocalListener::bind(&endpoint).unwrap().serve(host.clone(), stop.clone()));
            let mut peer = Peer::new(host, "provider-directory").await;
            ready(&mut peer).await;
            success(peer.rpc("plugin.package.install", json!({"sourcePath":source})).await);
            ready(&mut peer).await;
            let page = success(peer.rpc("model.provider.catalog.query", json!({})).await);
            let entry = page["entries"].as_array().unwrap().iter()
                .find(|entry| entry["identity"]["packageId"] == "example.account").unwrap();
            let identity = entry["identity"].clone();
            assert_eq!(identity, json!({
                "packageId":"example.account","entryId":"example.account","scope":"profile","name":"example.account",
            }));
            assert_eq!(entry["descriptor"]["authentication"][0]["id"], "key");
            assert_eq!(entry["descriptor"]["configurationDefaults"]["baseUrl"], "https://provider.invalid/v1");
            let revision = page["revision"].clone();
            for disabled in [true, false] {
                success(peer.rpc("plugin.composition.apply", json!({"operations":[{
                    "type":"update","entryId":"example.account","patch":{"disabled":disabled}
                }]})).await);
                ready(&mut peer).await;
                let stale = success(peer.rpc("model.provider.catalog.query", json!({"revision":revision})).await);
                assert_eq!(stale["kind"], "revision_changed");
                let page = success(peer.rpc("model.provider.catalog.query", json!({})).await);
                let entry = page["entries"].as_array().unwrap().iter().find(|entry| entry["identity"] == identity);
                assert_eq!(entry.is_none(), disabled);
            }
            peer.close().await;
            stop.cancel();
            server.await.unwrap().unwrap();
        }
    }).await.unwrap();
}
fn success(reply: Value) -> Value {
    assert_eq!(reply["ok"], true, "{reply}");
    reply["result"].clone()
}
