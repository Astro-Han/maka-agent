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

use super::{Peer, Value, json, remote, success};

#[derive(Default)]
pub(super) struct Probe {
    pub(super) grant: Value,
    cursor: Value,
}
impl Probe {
    pub(super) async fn check(
        &mut self,
        peer: &mut Peer,
        client: &Value,
        document: &Value,
        reopened: bool,
    ) {
        if !reopened {
            self.grant = approve(peer, client, "profile", None).await;
        } else {
            let stale = read(
                peer,
                client,
                document,
                &self.grant,
                json!({"kind":"continue", "cursor":self.cursor}),
            )
            .await;
            assert_eq!(
                stale["error"], "conflict",
                "a restarted Host invalidates page cursors"
            );
        }
        let start = json!({"kind":"start", "filter":{"from":0.0,"to":1e15}});
        let result = read(peer, client, document, &self.grant, start.clone()).await;
        let page = &result["page"];
        assert!(page["total"].as_u64().unwrap() > 0, "{result}");
        assert!(
            page["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["kind"] == "model")
        );
        for row in page["attempts"].as_array().unwrap() {
            let activity: maka_plugins::usage::Activity =
                serde_json::from_value(row.clone()).unwrap();
            if let maka_plugins::usage::Activity::Model(attempt) = activity {
                assert_eq!(attempt.cost_usd, None);
                assert_eq!(attempt.quote.unwrap().provider_id, "openai-compatible");
            }
        }
        let replay = read(
            peer,
            client,
            document,
            &self.grant,
            json!({"kind":"continue","cursor":page["cursor"]}),
        )
        .await;
        assert_eq!(replay, result, "re-reading preserves the same snapshot");
        self.cursor = page["cursor"].clone();

        // A valid maximum-size literal search must fit in a round-trippable
        // cursor even when JSON escaping doubles its encoded size.
        let empty = read(
            peer,
            client,
            document,
            &self.grant,
            json!({
                "kind":"refine", "cursor":self.cursor,
                "selection":{"search":"\"".repeat(1024),"kind":"tool","status":"rejected"}
            }),
        )
        .await;
        assert_eq!(empty["page"]["total"], 0, "{empty}");
        assert!(
            empty["summary"]["models"]["calls"].as_u64().unwrap() > 0,
            "activity filters must not erase headline usage: {empty}"
        );
        let repeated = read(
            peer,
            client,
            document,
            &self.grant,
            json!({
                "kind":"continue", "cursor":empty["page"]["cursor"]
            }),
        )
        .await;
        assert_eq!(repeated, empty);
        assert_eq!(
            empty["summary"], result["summary"],
            "refining activity retains the headline snapshot"
        );

        let session = approve(peer, client, "session", Some("background-session")).await;
        let scoped = read(peer, client, document, &session, start.clone()).await;
        assert!(
            scoped["page"]["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .all(|row| if row["kind"] == "model" {
                    row["attempt"]["sessionId"] == "background-session"
                } else {
                    row["attempt"]["invocation"]["session_id"] == "background-session"
                })
        );
        let denied = read(
            peer,
            client,
            document,
            &session,
            json!({"kind":"start","filter":{"from":0,"to":1e15,"sessionId":"foreign"}}),
        )
        .await;
        assert_eq!(denied["error"], "revoked");
        // A profile cursor is not a transferable profile grant.
        let denied = read(
            peer,
            client,
            document,
            &session,
            json!({"kind":"continue","cursor":self.cursor}),
        )
        .await;
        assert_eq!(denied["error"], "revoked");

        if reopened {
            success(peer.rpc("plugin.authorization", json!({
                "client":client,"scope":"profile","command":{"kind":"revoke","id":self.grant["id"]}
            })).await);
            let denied = read(peer, client, document, &self.grant, start).await;
            assert_eq!(denied["error"], "revoked");
        }
    }
}

async fn approve(peer: &mut Peer, client: &Value, kind: &str, session: Option<&str>) -> Value {
    let target = session.map_or_else(
        || json!({"kind":kind}),
        |id| json!({"kind":kind,"sessionId":id}),
    );
    success(
        peer.rpc(
            "plugin.authorization",
            json!({
                "client":client,"scope":"profile","command":{"kind":"approve","request":{
                    "operationId":uuid::Uuid::new_v4(),"title":"Read model usage",
                    "target":target,"capabilities":["read_usage"]
                }}
            }),
        )
        .await,
    )["grant"]
        .clone()
}
async fn read(
    peer: &mut Peer,
    client: &Value,
    document: &Value,
    grant: &Value,
    read: Value,
) -> Value {
    remote(
        peer,
        client,
        document,
        "usage",
        json!({"grant":grant["id"],"read":read}),
    )
    .await
}
