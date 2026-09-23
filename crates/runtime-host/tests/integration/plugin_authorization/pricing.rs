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

use super::{Host, Peer, Value, json, remote, success};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Probe {
    grant: Value,
}
impl Probe {
    pub(super) async fn check(
        &mut self,
        host: &Arc<Host>,
        peer: &mut Peer,
        client: &Value,
        document: &Value,
        read_only: &Value,
        reopened: bool,
    ) {
        let initial = remote(
            peer,
            client,
            document,
            "pricing",
            json!({"operation":"query"}),
        )
        .await;
        assert_eq!(initial["kind"], "page");
        let revision = initial["revision"].as_u64().unwrap();
        let model_key = "!acceptance:public-pricing";
        let price = json!({"modelKey":model_key,"inputUsdPer1M":1.5,"outputUsdPer1M":2.5});
        let upsert =
            json!({"expectedRevision":revision,"mutation":{"kind":"upsert","pricing":price}});
        let denied = edit(peer, client, document, read_only, upsert.clone()).await;
        assert_eq!(
            denied["error"], "revoked",
            "reading usage is not price-edit consent"
        );
        let existing = initial["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["pricing"]["modelKey"] == model_key);
        if reopened {
            assert_eq!(
                existing.unwrap()["pricing"],
                price,
                "public edits survive Host restart"
            );
        } else {
            assert!(existing.is_none());
            self.grant = success(
                peer.rpc(
                    "plugin.authorization",
                    json!({
                        "client":client,"scope":"profile","command":{"kind":"approve","request":{
                            "operationId":uuid::Uuid::new_v4(),"title":"Manage future model rates",
                            "target":{"kind":"profile"},"capabilities":["manage_pricing"]
                        }}
                    }),
                )
                .await,
            )["grant"]
                .clone();
        }
        let mut observer = Peer::new(host.clone(), "pricing-observer").await;
        let changed = if reopened {
            json!({"expectedRevision":revision,"mutation":{"kind":"delete","modelKey":model_key}})
        } else {
            upsert
        };
        let committed = edit(peer, client, document, &self.grant, changed.clone()).await;
        assert_eq!(committed, json!({"kind":"committed","revision":revision+1}));
        // Delivery invalidates the same native configuration view, not a private plugin catalog.
        loop {
            let notice = observer.frame().await;
            if notice["kind"] == "configuration.changed" {
                break;
            }
        }
        observer.close().await;
        let replay = edit(peer, client, document, &self.grant, changed).await;
        assert_eq!(
            replay,
            json!({"kind":"revision_conflict","expectedRevision":revision,"actualRevision":revision+1})
        );
        let current = success(peer.rpc("pricing.query", json!({"kind":"start"})).await);
        assert_eq!(current["revision"], revision + 1);
        let custom = current["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["pricing"]["modelKey"] == model_key);
        if reopened {
            assert!(custom.is_none());
            success(peer.rpc("plugin.authorization", json!({
                "client":client,"scope":"profile","command":{"kind":"revoke","id":self.grant["id"]}
            })).await);
            let denied = edit(peer, client, document, &self.grant,
                json!({"expectedRevision":revision+1,"mutation":{"kind":"delete","modelKey":model_key}})).await;
            assert_eq!(denied["error"], "revoked");
        } else {
            assert_eq!(custom.unwrap()["source"], "custom");
            assert_eq!(custom.unwrap()["resetEffect"], "become_unpriced");
        }
    }
}
async fn edit(
    peer: &mut Peer,
    client: &Value,
    document: &Value,
    grant: &Value,
    update: Value,
) -> Value {
    remote(
        peer,
        client,
        document,
        "pricing",
        json!({"operation":"update","grant":grant["id"],"update":update}),
    )
    .await
}
