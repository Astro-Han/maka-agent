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

use super::super::{
    javascript_plugins::{package, ready},
    support::{client_probe::ClientFixture, message_recovery, peer::Peer},
};
use maka_runtime::interaction::InteractionRequest;
use maka_runtime_host::server::{Host, local::LocalListener};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn agent_http_requires_approval_and_preserves_grant_scope_and_filesystem_boundary() {
    tokio::time::timeout(Duration::from_secs(30), scenario())
        .await
        .unwrap();
}

async fn scenario() {
    let fixture = ClientFixture::new("maka-http-permissions-");
    let (provider, mut requests) = message_recovery::Provider::controlled().await;
    let model = message_recovery::configure(&fixture, &provider.base_url).await;
    let received = Arc::new(AtomicUsize::new(0));
    let counted = received.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let cleanup = stop.clone().drop_guard();
    let stopped = stop.clone();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = stopped.cancelled() => break,
                accepted = listener.accept() => {
                    let (socket, _) = accepted.unwrap();
                    let counted = counted.clone();
                    connections.spawn(async move {
                        hyper::server::conn::http1::Builder::new().serve_connection(
                            hyper_util::rt::TokioIo::new(socket),
                            hyper::service::service_fn(move |_| {
                                counted.fetch_add(1, Ordering::SeqCst);
                                async { Ok::<_, std::convert::Infallible>(hyper::Response::new(
                                    http_body_util::Full::new(hyper::body::Bytes::from_static(b"network-approved"))
                                )) }
                            })
                        ).await.unwrap();
                    });
                }
                Some(result) = connections.join_next() => { result.unwrap(); }
            }
        }
    });
    let source = package(
        &fixture.workspace,
        "example.http-permissions",
        "shared",
        r#"
export default async function(ctx) {
  let previous;
  await ctx.tools.register({name: 'BoundaryWrite', description: 'Verify call boundary',
    directOnly: true, inputSchema: {type: 'object', properties: {wait: {type: 'boolean'}}}},
    async (input, call) => {
      if (input.wait) await call.llm.generate({prompt: 'call-boundary-gate'});
      try {
        await call.files.write({path: 'boundary-live.txt', content: 'new call'});
        await call.files.entries.write({path: 'boundary-live.txt', bytes: new TextEncoder().encode('new entries')});
        return {text: 'call-written'};
      } catch (error) {
        if (error.code !== 'revoked') throw error;
        return {text: 'call-denied'};
      }
    });
  await ctx.tools.register({name: 'RequestAccess', description: 'Request execution access',
    directOnly: true, inputSchema: {type: 'object'}},
    (input, call) => call.permissions.request(input));
  await ctx.executors.register({name: 'example.http-permissions', displayName: 'Network approval'}, async (request, call) => {
    await call.permissions.request({reason: 'Executor fetch', permissions: {filesystem: [], network: 'allowed'}});
    const response = await call.http.request({url: request.content.text});
    await response.close();
    return {status: 'completed', text: 'executor network approved'};
  });
  await ctx.tools.register({
    name: 'ApprovedHttp', description: 'Fetch with network approval', directOnly: true,
    inputSchema: {type: 'object', properties: {url: {type: 'string'}, native: {type: 'boolean'}, write: {type: 'boolean'}, retain: {type: 'boolean'}}, required: ['url'], additionalProperties: false}
  }, async (input, call) => {
    if (previous) {
      const [process, terminal] = previous;
      previous = undefined;
      for (const write of [
        () => call.processes.open(process.id).write('not authorized\n'),
        () => call.terminals.open(terminal.id).write('not authorized\r'),
      ]) {
        try { await write(); throw new Error('expired grant survived resource rebinding'); }
        catch (error) { if (error.code !== 'revoked') throw error; }
      }
      await process.close();
      await terminal.close();
    }
    try {
      const response = await call.http.request({url: input.url});
      let text = '';
      const decoder = new TextDecoder();
      try {
        for (let chunk = await response.next(); chunk !== null; chunk = await response.next())
          text += decoder.decode(chunk, {stream: true});
      } finally { await response.close(); }
      try {
        await call.files.write({path: 'forbidden', content: 'network is not file authority'});
        throw new Error('network grant widened filesystem');
      } catch (error) { if (error.code !== 'revoked') throw error; }
      if (input.write) {
        await call.files.write({path: 'granted.txt', content: 'approved text'});
        await call.files.entries.write({path: 'granted.txt', bytes: new TextEncoder().encode('approved entries')});
      }
      if (input.native) {
        // A network grant is shared by SDK HTTP, pipes and PTYs, not a mode change.
        const command = {executable: '/usr/bin/curl', args: ['-fsS', '--max-time', '3', input.url], env: {}};
        const process = await call.processes.spawn(command);
        let output = '';
        try {
          for (let chunk = await process.next(); chunk !== null; chunk = await process.next())
            output += decoder.decode(chunk.bytes, {stream: true});
          if (!(await process.wait()).success || output !== text) throw new Error(`pipe grant lost: ${output}`);
        } finally { await process.close(); }
        const terminal = await call.terminals.spawn(command);
        try {
          if ((await terminal.wait()).kind !== 'completed') throw new Error('PTY grant lost');
        } finally { await terminal.close(); }
        if (input.retain) {
          const idle = {executable: '/bin/cat', args: [], env: {}, lifetime: 'instance'};
          previous = [await call.processes.spawn(idle), await call.terminals.spawn(idle)];
        }
      }
      return {text};
    } catch (error) {
      if (error.code === 'revoked' && !input.write) return {text: 'network-denied'};
      throw error;
    }
  });
}
"#,
        false,
    );
    let host = Host::open(fixture.owner()).await.unwrap();
    #[cfg(unix)]
    let endpoint = fixture
        .workspace
        .parent()
        .unwrap()
        .join("http-approval.sock");
    #[cfg(windows)]
    let endpoint = std::path::PathBuf::from(format!(
        r"\\.\pipe\maka-http-approval-{}",
        uuid::Uuid::new_v4()
    ));
    let host_server = tokio::spawn(
        LocalListener::bind(&endpoint)
            .unwrap()
            .serve(host.clone(), stop.clone()),
    );
    let mut peer = Peer::new(host.clone(), "http-permissions").await;
    ready(&mut peer).await;
    let installed = peer
        .rpc("plugin.package.install", json!({"sourcePath":source}))
        .await;
    assert_eq!(installed["ok"], true, "{installed}");
    ready(&mut peer).await;
    let created = peer.rpc("session.create", json!({
        "sessionId":"network", "workspace":{"kind":"host_path","path":fixture.workspace},
        "sandboxMode":"read-only", "approvalPolicy":{"kind":"on-request"},
        "modelTarget":{"kind":"explicit","connectionId":model.connection_id,"connectionSlug":model.connection_slug,"model":model.model}
    })).await;
    assert_eq!(created["ok"], true, "{created}");
    // OS-backed SDK launches are covered here on macOS; HTTP itself is portable.
    let native = cfg!(target_os = "macos");
    let requests_per_call = if native { 3 } else { 1 };
    for turn in 0..4 {
        if turn >= 2 {
            let state = peer
                .rpc(
                    "session.catalog.query",
                    json!({"kind":"get","sessionId":"network"}),
                )
                .await;
            let changed = peer.rpc("session.configuration.update", json!({
                "sessionId":"network", "expectedRevision":state["result"]["session"]["revision"],
                "patch":{"approvalPolicy":{"kind":if turn == 2 { "never" } else { "on-request" }}}
            })).await;
            assert_eq!(changed["result"]["kind"], "committed", "{changed}");
        }
        let turn_id = format!("network-{turn}");
        let started = peer
            .rpc(
                "turn.start",
                json!({"sessionId":"network","turnId":turn_id,"content":{"text":"fetch"}}),
            )
            .await;
        assert_eq!(started["ok"], true, "{started}");
        tool(
            requests.recv().await.unwrap(),
            &format!("search-{turn}"),
            "tool_search",
            json!({"query":if turn == 3 { "ApprovedHttp RequestAccess" } else { "ApprovedHttp" }}),
        );
        let mut request = requests.recv().await.unwrap();
        if turn == 3 {
            let permissions = maka_sandbox::grant::Permissions {
                filesystem: vec![maka_sandbox::filesystem::Rule::exact(
                    fixture.workspace.join("granted.txt"),
                    maka_sandbox::filesystem::Access::Write,
                )],
                network: maka_sandbox::Network::Allowed,
            };
            tool(
                request,
                "file-grant",
                "RequestAccess",
                json!({
                    "permissions":permissions,
                    "reason":"Write only the approved file and fetch over the network"
                }),
            );
            let waiting = async {
                loop {
                    if let Some(approval) = pending(&mut peer).await.into_iter().next() {
                        break approval;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            };
            let approval = tokio::select! {
                approval = waiting => approval,
                next = requests.recv() => panic!("SDK continued without approval: {}", next.unwrap().body["messages"].as_array().unwrap().last().unwrap()),
            };
            let InteractionRequest::Permissions {
                request: resolved, ..
            } = approval.request()
            else {
                panic!("expected additional access approval");
            };
            let canonical = fixture.workspace.canonicalize().unwrap();
            let canonical = maka_fs_tools::workspace::project::host_path(&canonical).unwrap();
            assert_eq!(
                resolved.permissions.filesystem[0].path,
                std::path::Path::new(canonical).join("granted.txt")
            );
            let answered = peer.rpc("interaction.answer", json!({
                "sessionId":"network", "interactionId":approval.interaction_id(),
                "answer":{"kind":"permissions","decision":{"decision":"allow","permissions":resolved.permissions,"scope":"turn"}}
            })).await;
            assert_eq!(answered["ok"], true, "{answered}");
            request = requests.recv().await.unwrap();
        }
        for call in 0..if turn == 0 { 3 } else { 1 } {
            tool(
                request,
                &format!("http-{turn}-{call}"),
                "ApprovedHttp",
                json!({"url":url,"native":native,"write":turn == 3,"retain":turn == 0 && call == 0}),
            );
            if turn < 2 && call < 2 {
                let waiting = async {
                    loop {
                        let pending = pending(&mut peer).await;
                        if let Some(pending) = pending.into_iter().next() {
                            break pending;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                };
                let pending = tokio::select! {
                    pending = waiting => pending,
                    next = requests.recv() => panic!("HTTP continued without approval: {}", next.unwrap().body),
                };
                let InteractionRequest::Permissions {
                    request: permissions,
                    ..
                } = pending.request()
                else {
                    panic!("expected network approval")
                };
                assert!(permissions.permissions.filesystem.is_empty());
                assert_eq!(
                    permissions.permissions.network,
                    maka_sandbox::Network::destination(
                        maka_sandbox::Destination::new(
                            "127.0.0.1",
                            reqwest::Url::parse(&url).unwrap().port().unwrap()
                        )
                        .unwrap()
                    )
                );
                assert_eq!(
                    received.load(Ordering::SeqCst),
                    if turn == 0 { call } else { 3 } * requests_per_call
                );
                let decision = if turn == 1 {
                    json!({"decision":"deny"})
                } else {
                    json!({"decision":"allow", "permissions":permissions.permissions, "scope":if call == 0 { "once" } else { "turn" }})
                };
                let answered = peer
                    .rpc(
                        "interaction.answer",
                        json!({
                        "sessionId":"network","interactionId":pending.interaction_id(),
                                "answer":{"kind":"permissions","decision":decision}
                            }),
                    )
                    .await;
                assert_eq!(answered["ok"], true, "{answered}");
            }
            request = requests.recv().await.unwrap();
            let result = request.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|message| message["role"] == "tool")
                .unwrap();
            assert!(
                result.to_string().contains(if turn == 0 || turn == 3 {
                    "network-approved"
                } else {
                    "network-denied"
                }),
                "{result}"
            );
            assert!(pending(&mut peer).await.is_empty());
        }
        request
            .reply
            .send(json!({"index":0,"delta":{"content":"done"},"finish_reason":"stop"}))
            .unwrap();
        loop {
            let state = peer
                .rpc(
                    "turn.query",
                    json!({"sessionId":"network","turnId":turn_id}),
                )
                .await;
            if state["result"]["status"] == "completed" {
                break;
            }
            assert!(
                matches!(
                    state["result"]["status"].as_str(),
                    Some("admitted" | "created" | "running")
                ),
                "{state}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    assert_eq!(received.load(Ordering::SeqCst), 4 * requests_per_call);
    assert!(!fixture.workspace.join("forbidden").exists());
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("granted.txt")).unwrap(),
        "approved entries"
    );
    live_boundary(&mut peer, &mut requests, &fixture.workspace).await;
    let created = peer
        .rpc(
            "session.create",
            json!({
                "sessionId":"executor", "workspace":{"kind":"host_path","path":fixture.workspace},
                "sandboxMode":"read-only", "approvalPolicy":{"kind":"on-request"},
                "executorId":"example.http-permissions"
            }),
        )
        .await;
    assert_eq!(created["ok"], true, "{created}");
    let started = peer
        .rpc(
            "turn.start",
            json!({
                "sessionId":"executor","turnId":"executor-turn","content":{"text":url}
            }),
        )
        .await;
    assert_eq!(started["ok"], true, "{started}");
    let approval = loop {
        if let Some(approval) = pending_for(&mut peer, "executor").await.into_iter().next() {
            break approval;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(matches!(
        approval.request(),
        InteractionRequest::Permissions {
            tool_use_id: None,
            ..
        }
    ));
    let answer = |scope| {
        json!({"sessionId":"executor", "interactionId":approval.interaction_id(),
        "answer":{"kind":"permissions","decision":{"decision":"allow","scope":scope,
            "permissions":{"filesystem":[],"network":"allowed"}}}})
    };
    let rejected = peer.rpc("interaction.answer", answer("once")).await;
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert_eq!(received.load(Ordering::SeqCst), 4 * requests_per_call);
    let accepted = peer.rpc("interaction.answer", answer("turn")).await;
    assert_eq!(accepted["ok"], true, "{accepted}");
    loop {
        let state = peer
            .rpc(
                "turn.query",
                json!({"sessionId":"executor","turnId":"executor-turn"}),
            )
            .await;
        if state["result"]["status"] == "completed" {
            break;
        }
        assert!(
            matches!(
                state["result"]["status"].as_str(),
                Some("admitted" | "created" | "running")
            ),
            "{state}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(received.load(Ordering::SeqCst), 4 * requests_per_call + 1);
    // Retiring a tool provider must close an unanswered approval before draining
    // the active call. Human input cannot be required to unload a plugin.
    let started = peer
        .rpc(
            "turn.start",
            json!({
                "sessionId":"network", "turnId":"retire-pending", "content":{"text":"fetch"}
            }),
        )
        .await;
    assert_eq!(started["ok"], true, "{started}");
    tool(
        requests.recv().await.unwrap(),
        "find-retiring",
        "tool_search",
        json!({"query":"ApprovedHttp"}),
    );
    tool(
        requests.recv().await.unwrap(),
        "retiring-http",
        "ApprovedHttp",
        json!({"url":url}),
    );
    while pending(&mut peer).await.is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let disabled = peer.rpc("plugin.composition.apply", json!({
        "operations":[{"type":"update","entryId":"example.http-permissions","patch":{"disabled":true}}]
    })).await;
    assert_eq!(disabled["ok"], true, "{disabled}");
    let continued = requests.recv().await.unwrap();
    assert!(pending(&mut peer).await.is_empty());
    assert_eq!(received.load(Ordering::SeqCst), 4 * requests_per_call + 1);
    ready(&mut peer).await;
    continued
        .reply
        .send(json!({"index":0,"delta":{"content":"plugin retired"},"finish_reason":"stop"}))
        .unwrap();
    peer.close().await;
    stop.cancel();
    host_server.await.unwrap().unwrap();
    server.await.unwrap();
    cleanup.disarm();
}

async fn live_boundary(
    peer: &mut Peer,
    requests: &mut tokio::sync::mpsc::Receiver<message_recovery::ModelRequest>,
    workspace: &std::path::Path,
) {
    let started = peer.rpc("turn.start", json!({
        "sessionId":"network", "turnId":"live-boundary", "content":{"text":"change permissions"}
    })).await;
    assert_eq!(started["ok"], true, "{started}");
    tool(
        requests.recv().await.unwrap(),
        "find-boundary",
        "tool_search",
        json!({"query":"BoundaryWrite"}),
    );
    tool(
        requests.recv().await.unwrap(),
        "old-boundary",
        "BoundaryWrite",
        json!({"wait":true}),
    );
    let nested = requests.recv().await.unwrap();
    assert!(
        nested.body["messages"]
            .to_string()
            .contains("call-boundary-gate"),
        "{}",
        nested.body
    );
    change_mode(peer, "workspace-write").await;
    // The SDK model request is already accepted and may settle, but the same
    // plugin call must not acquire the newly expanded filesystem authority.
    nested
        .reply
        .send(json!({"index":0,"delta":{"content":"release"},"finish_reason":"stop"}))
        .unwrap();
    let next = requests.recv().await.unwrap();
    assert!(
        next.body["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .to_string()
            .contains("call-denied"),
        "{}",
        next.body
    );
    assert!(!workspace.join("boundary-live.txt").exists());
    tool(next, "new-boundary", "BoundaryWrite", json!({"wait":false}));
    let next = requests.recv().await.unwrap();
    assert!(
        next.body["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .to_string()
            .contains("call-written"),
        "{}",
        next.body
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("boundary-live.txt")).unwrap(),
        "new entries"
    );
    next.reply
        .send(json!({"index":0,"delta":{"content":"done"},"finish_reason":"stop"}))
        .unwrap();
    loop {
        let state = peer
            .rpc(
                "turn.query",
                json!({"sessionId":"network","turnId":"live-boundary"}),
            )
            .await;
        if state["result"]["status"] == "completed" {
            break;
        }
        assert!(
            matches!(
                state["result"]["status"].as_str(),
                Some("admitted" | "created" | "running")
            ),
            "{state}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    change_mode(peer, "read-only").await;
}

async fn change_mode(peer: &mut Peer, mode: &str) {
    let state = peer
        .rpc(
            "session.catalog.query",
            json!({"kind":"get","sessionId":"network"}),
        )
        .await;
    let changed = peer
        .rpc(
            "session.configuration.update",
            json!({
                "sessionId":"network", "expectedRevision":state["result"]["session"]["revision"],
                "patch":{"sandboxMode":mode}
            }),
        )
        .await;
    assert_eq!(changed["result"]["kind"], "committed", "{changed}");
}

async fn pending(peer: &mut Peer) -> Vec<maka_protocol::interaction::InteractionSnapshot> {
    pending_for(peer, "network").await
}
async fn pending_for(
    peer: &mut Peer,
    session: &str,
) -> Vec<maka_protocol::interaction::InteractionSnapshot> {
    let opened = peer
        .rpc(
            "subscription.open",
            json!({"sessionId":session,"transcript":{"kind":"none"}}),
        )
        .await;
    assert_eq!(opened["ok"], true, "{opened}");
    let closed = peer
        .rpc(
            "subscription.close",
            json!({"subscriptionId":opened["result"]["subscriptionId"]}),
        )
        .await;
    assert_eq!(closed["ok"], true, "{closed}");
    opened["result"]["snapshot"]["interactions"]["pending"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| maka_protocol::interaction::decode_snapshot(value).unwrap())
        .collect()
}

fn tool(request: message_recovery::ModelRequest, id: &str, name: &str, input: Value) {
    request
        .reply
        .send(json!({"index":0,"delta":{"tool_calls":[{
        "index":0,"id":id,"type":"function","function":{"name":name,"arguments":input.to_string()}
    }]},"finish_reason":"tool_calls"}))
        .unwrap();
}
