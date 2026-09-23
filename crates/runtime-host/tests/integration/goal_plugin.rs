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
    javascript_plugins::ready,
    support::{
        client_probe::ClientFixture,
        message_recovery::{ModelRequest, Provider, configure},
        peer::Peer,
    },
};
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

async fn binding(peer: &mut Peer) -> Value {
    let snapshot = peer
        .rpc("plugin.client.query", json!({"kind":"snapshot"}))
        .await;
    let entry = snapshot["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["extensionId"] == "maka.goal")
        .unwrap();
    json!({"client":{
        "entryId":entry["entryId"],"extensionId":entry["extensionId"],
        "activation":entry["activation"],"contentDigest":entry["contentDigest"],
        "clientDigest":entry["clientDigest"]
    },"method":"request","sessionId":"goal-session"})
}

async fn open(peer: &mut Peer) -> Value {
    let binding = binding(peer).await;
    let bound = peer
        .rpc("plugin.remote", json!({"kind":"bind","binding":binding}))
        .await;
    assert_eq!(bound["ok"], true, "{bound}");
    let opened = peer
        .rpc("plugin.remote", json!({"kind":"open_document"}))
        .await;
    assert_eq!(opened["ok"], true, "{opened}");
    json!({"kind":"call","document":opened["result"]["document"],
        "binding":binding,"target":bound["result"]["target"]})
}
async fn call(peer: &mut Peer, envelope: &Value, input: Value) -> Value {
    let mut call = envelope.clone();
    call["input"] = input;
    peer.rpc("plugin.remote", call).await
}
async fn close(peer: &mut Peer, envelope: &Value) {
    let response = peer
        .rpc(
            "plugin.remote",
            json!({
                "kind":"close_document","document":envelope["document"]
            }),
        )
        .await;
    assert_eq!(response["ok"], true, "{response}");
}
async fn disable(peer: &mut Peer, disabled: bool) {
    let result = peer
        .rpc(
            "plugin.composition.apply",
            json!({"operations":[
                {"type":"update","entryId":"maka.goal","patch":{"disabled":disabled}}
            ]}),
        )
        .await;
    assert_eq!(result["ok"], true, "{result}");
    if !disabled {
        ready(peer).await;
    } else {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let clients = peer
                    .rpc("plugin.client.query", json!({"kind":"snapshot"}))
                    .await;
                if !clients["result"]["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["extensionId"] == "maka.goal")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

fn answer(text: &str) -> Value {
    json!({"index":0,"delta":{"content":text},"finish_reason":"stop"})
}
fn tool(id: &str, status: &str, note: &str) -> Value {
    json!({"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":"GoalStatus","arguments":json!({"status":status,"note":note}).to_string()}}]},"finish_reason":"tool_calls"})
}
async fn next(requests: &mut tokio::sync::mpsc::Receiver<ModelRequest>) -> ModelRequest {
    tokio::time::timeout(Duration::from_secs(8), requests.recv())
        .await
        .unwrap()
        .unwrap()
}
async fn current(peer: &mut Peer, envelope: &Value) -> Value {
    let r = call(peer, envelope, json!({"kind":"read"})).await;
    assert_eq!(r["ok"], true, "{r}");
    r["result"]["value"]["current"].clone()
}
async fn control(peer: &mut Peer, envelope: &Value, action: &str) -> Value {
    let c = current(peer, envelope).await;
    let r = call(
        peer,
        envelope,
        json!({"kind":"control","id":c["goal"]["id"],"revision":c["revision"],"action":action}),
    )
    .await;
    assert_eq!(r["ok"], true, "{r}");
    r
}
async fn settled(peer: &mut Peer, envelope: &Value, status: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let c = current(peer, envelope).await;
            if c["goal"]["status"] == status && c["goal"]["pending"].is_null() {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn goal_recovers_authority_continues_and_stops_with_exact_control_and_usage() {
    tokio::time::timeout(Duration::from_secs(90), scenario())
        .await
        .unwrap();
}
async fn scenario() {
    let fixture = ClientFixture::new("maka-goal-");
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&fixture.workspace)
            .status()
            .unwrap()
            .success()
    );
    let (provider, mut requests) = Provider::controlled_with_usage(3, 5).await;
    let model = configure(&fixture, &provider.base_url).await;
    let id = uuid::Uuid::new_v4();
    let mut grant = Value::Null;
    let mut arm = Value::Null;
    for phase in 0..3 {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("goal.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-goal-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop),
        );
        let mut peer = Peer::new(host.clone(), "goal").await;
        ready(&mut peer).await;
        if phase == 0 {
            let created=peer.rpc("session.create",json!({"sessionId":"goal-session","workspace":{"kind":"host_path","path":fixture.workspace},"sandboxMode":"danger-full-access","approvalPolicy":{"kind":"never"},"modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model}})).await;
            assert_eq!(created["ok"], true, "{created}");
            let client = binding(&mut peer).await["client"].clone();
            let approved=peer.rpc("plugin.authorization",json!({"client":client,"scope":"profile","command":{"kind":"approve","request":{"operationId":uuid::Uuid::new_v4(),"title":"Continue this Goal","target":{"kind":"session","sessionId":"goal-session"},"capabilities":["executions","read_usage"]}}})).await;
            assert_eq!(approved["ok"], true, "{approved}");
            grant = approved["result"]["grant"]["id"].clone();
            arm = json!({"kind":"arm","arm":{"operationId":id,"objective":"Verify the goal integration","grant":grant,"maxIterations":3,"tokenBudget":null,"start":false}});
        }
        let mut envelope = open(&mut peer).await;
        if phase == 2 {
            let recovered = settled(&mut peer, &envelope, "blocked").await;
            assert_eq!(recovered["goal"]["iterations"], 1);
            assert!(
                requests.try_recv().is_err(),
                "Restart must reconcile the cancelled original operation, not submit a replacement"
            );
            control(&mut peer, &envelope, "resume").await;
            next(&mut requests)
                .await
                .reply
                .send(answer("resumed progress"))
                .unwrap();
            let limited = settled(&mut peer, &envelope, "max_iterations").await;
            assert_eq!(limited["goal"]["iterations"], 2);
            let rejected=call(&mut peer,&envelope,json!({"kind":"control","id":limited["goal"]["id"],"revision":limited["revision"],"action":"complete"})).await;
            assert_eq!(
                rejected["ok"], false,
                "Terminal states must be monotonic: {rejected}"
            );
            let uncertain=call(&mut peer,&envelope,json!({"kind":"arm","arm":{"operationId":uuid::Uuid::new_v4(),"objective":"Recover a dispatch commitment with no known receipt","grant":grant,"maxIterations":2,"tokenBudget":null,"start":false}})).await;
            assert_eq!(uncertain["ok"], true, "{uncertain}");
            disable(&mut peer, true).await;
            // Restore the precise durable state possible after a crash between
            // committing the dispatch marker and receiving Host acceptance.
            let inspector = maka_plugins::fiber::Fiber::new(
                "maka.goal",
                "goal-fixture",
                maka_plugins::composition::Scope::Profile,
            )
            .unwrap();
            inspector.begin_loading().unwrap();
            inspector.ready().unwrap();
            inspector.publish().unwrap();
            let store = host.plugin_storage(inspector.context()).unwrap();
            let page = store
                .scan(maka_plugins::storage::Scan {
                    prefix: "session/".into(),
                    after: None,
                })
                .await
                .unwrap();
            let entry = &page.entries[0];
            let mut value = entry.record.data.value().unwrap().clone();
            let request = maka_plugins::execution::Submit {
                operation_id: "unknown-dispatch-fixture".into(),
                session_id: "goal-session".into(),
                content: "This cancelled intent must never be submitted".into(),
                orchestration_mode: None,
            };
            value["pending"] = json!({"request":request,"dispatched":true});
            value["status"] = json!("cancelled");
            value["iterations"] = json!(1);
            store
                .batch(vec![maka_plugins::storage::Mutation {
                    key: entry.key.clone(),
                    expected_revision: Some(entry.record.revision),
                    data: maka_plugins::storage::Data::Present(value),
                }])
                .await
                .unwrap();
            disable(&mut peer, false).await;
            close(&mut peer, &envelope).await;
            envelope = open(&mut peer).await;
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let c = current(&mut peer, &envelope).await;
                    if c["goal"]["status"] == "cancellation_unknown" {
                        assert!(c["goal"]["pending"].is_object());
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .unwrap();
            assert!(
                requests.try_recv().is_err(),
                "Unknown cancelled admission must not be retried"
            );
            close(&mut peer, &envelope).await;
            peer.close().await;
            tokio::time::timeout(
                Duration::from_secs(5),
                host.wait_until_idle(Duration::ZERO, Duration::from_millis(100)),
            )
            .await
            .expect("Unknown cancellation must not pin idle Host");
            drop(cleanup);
            server.await.unwrap().unwrap();
            continue;
        }
        let armed = call(&mut peer, &envelope, arm.clone()).await;
        assert_eq!(armed["ok"], true, "{armed}");
        assert_eq!(
            armed["result"]["value"]["current"]["goal"]["status"], "armed",
            "{armed}"
        );
        assert!(
            requests.try_recv().is_err(),
            "Arm must not dispatch until explicitly started"
        );
        if phase == 1 {
            control(&mut peer, &envelope, "resume").await;
            let first = next(&mut requests).await;
            first
                .reply
                .send(tool("progress", "progress", "First part verified"))
                .unwrap();
            let finish = next(&mut requests).await;
            // Pausing does not cancel the accepted invocation, and blocks further iterations.
            control(&mut peer, &envelope, "pause").await;
            finish.reply.send(answer("first iteration done")).unwrap();
            let paused = settled(&mut peer, &envelope, "paused").await;
            assert_eq!(paused["goal"]["iterations"], 1);
            assert!(requests.try_recv().is_err());
            // A stale UI revision cannot resume a different state.
            let stale = call(
                &mut peer,
                &envelope,
                json!({"kind":"control","id":id,"revision":1,"action":"resume"}),
            )
            .await;
            assert_eq!(stale["ok"], false);
            control(&mut peer, &envelope, "resume").await;
            let second = next(&mut requests).await;
            second
                .reply
                .send(tool("done", "achieved", "All focused tests passed"))
                .unwrap();
            let finish = next(&mut requests).await;
            let before = current(&mut peer, &envelope).await;
            assert_eq!(before["goal"]["status"], "active");
            finish.reply.send(answer("verified")).unwrap();
            let completed = settled(&mut peer, &envelope, "achieved").await;
            assert_eq!(completed["goal"]["iterations"], 2);
            assert_eq!(completed["goal"]["consumed"]["known"], 32);
            // Retrying the original create must never start another execution.
            let duplicate = call(&mut peer, &envelope, arm.clone()).await;
            assert_eq!(duplicate["ok"], true, "{duplicate}");
            assert!(requests.try_recv().is_err());
            let threshold=call(&mut peer,&envelope,json!({"kind":"arm","arm":{"operationId":uuid::Uuid::new_v4(),"objective":"Check observed budget","grant":grant,"maxIterations":10,"tokenBudget":1,"start":true}})).await;
            assert_eq!(threshold["ok"], true, "{threshold}");
            next(&mut requests)
                .await
                .reply
                .send(answer("some progress"))
                .unwrap();
            let limited = settled(&mut peer, &envelope, "budget_limited").await;
            assert_eq!(limited["goal"]["iterations"], 1);
            assert_eq!(limited["goal"]["consumed"]["known"], 8);
            let cancel=call(&mut peer,&envelope,json!({"kind":"arm","arm":{"operationId":uuid::Uuid::new_v4(),"objective":"Cancel this owned execution","grant":grant,"maxIterations":2,"tokenBudget":null,"start":false}})).await;
            assert_eq!(cancel["ok"], true, "{cancel}");
            let client = binding(&mut peer).await["client"].clone();
            let revoked = peer.rpc("plugin.authorization", json!({"client":client,"scope":"profile","command":{"kind":"revoke","id":grant}})).await;
            assert_eq!(revoked["ok"], true, "{revoked}");
            let c = current(&mut peer, &envelope).await;
            let denied = call(&mut peer, &envelope, json!({"kind":"control","id":c["goal"]["id"],"revision":c["revision"],"action":"resume"})).await;
            assert_eq!(denied["ok"], false, "{denied}");
            assert!(requests.try_recv().is_err());
            let approved = peer.rpc("plugin.authorization", json!({"client":client,"scope":"profile","command":{"kind":"approve","request":{"operationId":uuid::Uuid::new_v4(),"title":"Renew Goal authority","target":{"kind":"session","sessionId":"goal-session"},"capabilities":["executions","read_usage"]}}})).await;
            assert_eq!(approved["ok"], true, "{approved}");
            grant = approved["result"]["grant"]["id"].clone();
            let resumed = call(&mut peer, &envelope, json!({"kind":"control","id":c["goal"]["id"],"revision":c["revision"],"action":"resume","grant":grant})).await;
            assert_eq!(resumed["ok"], true, "{resumed}");
            let in_flight = next(&mut requests).await;
            control(&mut peer, &envelope, "cancel").await;
            let cancelled = settled(&mut peer, &envelope, "cancelled").await;
            assert_eq!(cancelled["goal"]["iterations"], 1);
            let _ = in_flight.reply.send(answer("late"));
            disable(&mut peer, true).await;
            let retired = call(&mut peer, &envelope, json!({"kind":"read"})).await;
            assert_eq!(retired["ok"], false);
            disable(&mut peer, false).await;
            close(&mut peer, &envelope).await;
            envelope = open(&mut peer).await;
            let restart = call(&mut peer, &envelope, json!({"kind":"arm","arm":{"operationId":uuid::Uuid::new_v4(),"objective":"Recover the exact operation after Host restart","grant":grant,"maxIterations":2,"tokenBudget":null,"start":true}})).await;
            assert_eq!(restart["ok"], true, "{restart}");
            let waiting = next(&mut requests).await;
            // Keep the provider reply alive until Host has cancelled its accepted run.
            close(&mut peer, &envelope).await;
            peer.close().await;
            drop(cleanup);
            server.await.unwrap().unwrap();
            let _ = waiting.reply.send(answer("too late"));
            continue;
        }
        close(&mut peer, &envelope).await;
        peer.close().await;
        drop(cleanup);
        server.await.unwrap().unwrap();
    }
}
