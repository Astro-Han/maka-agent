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

use crate::support::peer::Peer;
use maka_client::{Client, ClientError, RequestFailure};
use maka_protocol::{Operation, OperationErrorCode, plugin};
use maka_runtime_host::server::Host;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

pub(super) async fn verify(host: Arc<Host>, endpoint: &Path) {
    let (peer, hello) = Peer::handshake(host.clone(), "remote-socket-bootstrap").await;
    peer.close().await;
    let (client, _notices) = Client::connect(
        maka_client::local::open_stream(endpoint).await.unwrap(),
        host.root_id(),
        hello["hostEpoch"].as_str().unwrap(),
        maka_client::Operations,
    )
    .await
    .unwrap();
    let snapshot = serde_json::to_value(
        client
            .plugin_clients(plugin::ClientQuery::Snapshot { cursor: None })
            .await
            .unwrap(),
    )
    .unwrap();
    let descriptor = snapshot["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["entryId"] == "remote-ui")
        .unwrap();
    let identity = json!({
        "entryId":descriptor["entryId"],"extensionId":descriptor["extensionId"],
        "activation":descriptor["activation"],"contentDigest":descriptor["contentDigest"],
        "clientDigest":descriptor["clientDigest"]
    });
    let document = remote(&client, json!({"kind":"open_document"})).await["document"].clone();
    let (binding, target) = bind(&client, &identity, "echo").await;
    let request =
        json!({"kind":"call","binding":binding,"target":target,"document":document,"input":null});
    assert_eq!(
        remote(&client, request.clone()).await["value"]["generation"],
        1
    );
    let (replace, replacement) = bind(&client, &identity, "replace").await;
    remote(&client, json!({"kind":"call","binding":replace,"target":replacement,"document":document,"input":null})).await;
    conflict(client.request(Operation::PluginRemote, request).await);
    let (binding, target) = bind(&client, &identity, "echo").await;
    assert_eq!(remote(&client, json!({"kind":"call","binding":binding,"target":target,"document":document,"input":null})).await["value"]["generation"], 2);

    let (binding, target) = bind(&client, &identity, "events").await;
    let stream = remote(
        &client,
        json!({"kind":"open","binding":binding,"target":target,"document":document,"input":null}),
    )
    .await["stream"]
        .clone();
    assert_eq!(
        remote(
            &client,
            json!({"kind":"next","document":document,"stream":stream})
        )
        .await,
        json!({"kind":"item","item":null})
    );
    let (read, closed) = tokio::join!(
        client.request(
            Operation::PluginRemote,
            json!({"kind":"next","document":document,"stream":stream})
        ),
        client.request(
            Operation::PluginRemote,
            json!({"kind":"close","document":document,"stream":stream})
        )
    );
    assert_eq!(closed.unwrap(), json!({"kind":"closed"}));
    match read {
        Ok(result) => assert_eq!(result, json!({"kind":"end"})),
        Err(error) => conflict(Err(error)),
    }
    let (binding, target) = bind(&client, &identity, "stats").await;
    let stats = remote(
        &client,
        json!({"kind":"call","binding":binding,"target":target,"document":document,"input":null}),
    )
    .await;
    assert_eq!(
        stats["value"],
        json!({"generation":2,"opening":0,"active":0,"stopped":3})
    );
    assert_eq!(
        remote(
            &client,
            json!({"kind":"close_document","document":document})
        )
        .await,
        json!({"kind":"closed"})
    );
    client.disconnect();
}
async fn remote(client: &Client, input: Value) -> Value {
    serde_json::to_value(
        client
            .plugin_remote(serde_json::from_value(input).unwrap())
            .await
            .unwrap(),
    )
    .unwrap()
}
async fn bind(client: &Client, identity: &Value, method: &str) -> (Value, Value) {
    let binding = json!({"client":identity,"method":method,"sessionId":null});
    let target = remote(client, json!({"kind":"bind","binding":binding})).await["target"].clone();
    (binding, target)
}
fn conflict(result: std::result::Result<Value, RequestFailure>) {
    assert!(
        matches!(&result, Err(RequestFailure::Rejected(ClientError::Rejected(error)))
            if error.code == OperationErrorCode::OperationConflict),
        "{result:?}"
    );
}
