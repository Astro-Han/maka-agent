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

use crate::support::{
    agent_loop::{Effect, input, respond},
    http::read_request,
};
use maka_agent::Engine;
use maka_event_log::EventLog;
use maka_js_runtime::{CellLimits, CodeExecutor};
use maka_model::ModelExecutor;
use maka_runtime::{
    event::{Fact, InvocationOutcome, TerminalStatus},
    handoff::HandoffIntent,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

fn intent(id: &str) -> HandoffIntent {
    HandoffIntent {
        handoff_id: id.into(),
        host_epoch: "host".into(),
        root_run_id: "run-first".into(),
        successor_run_id: format!("run-{id}"),
        successor_invocation_id: format!("invocation-{id}"),
        claim_id: format!("claim-{id}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handoff_waits_for_settlement_rollback_keeps_run_and_seal_survives_reopen() {
    tokio::time::timeout(Duration::from_secs(20), async {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.sqlite");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let (first_seen, first) = oneshot::channel();
        let (release_first, released_first) = oneshot::channel();
        let (second_seen, second) = oneshot::channel();
        let (release_second, released_second) = oneshot::channel();
        let (third_seen, mut third) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for (seen, release) in [(first_seen, released_first), (second_seen, released_second)] {
                let (mut socket, _) = listener.accept().await.unwrap();
                requests.push(read_request(&mut socket).await);
                seen.send(()).unwrap();
                release.await.unwrap();
                crate::support::agent_loop::respond_tool(&mut socket,
                    Some(if requests.len() == 1 { "echo" } else { "unavailable" })).await;
            }
            let (mut socket, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut socket).await);
            third_seen.send(()).unwrap();
            respond(&mut socket, false).await;
            requests
        });
        let log = Arc::new(EventLog::open(&path).await.unwrap());
        let engine = Engine::new(log.clone(), ModelExecutor::new(1, Duration::from_secs(5)).unwrap(),
            CodeExecutor::new(1, CellLimits::default()).unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let effect = Arc::new(Effect { log: log.clone(), count: count.clone() });
        let mut run = input(&base, "first", effect);
        run.configuration.workspace_identity = Some(maka_runtime::execution::WorkspaceIdentity::from_marker_id(
            "ef751105-55b5-4d65-a364-646281586a17").unwrap());
        run.main_output_limit = Some(120);
        let configuration = run.configuration.clone();
        let running = engine.start(run, CancellationToken::new()).await.unwrap();
        first.await.unwrap();
        let gate = running.handoff().unwrap().clone();
        let reservation = gate.reserve(intent("cancelled")).unwrap();
        let ready = reservation.ready();
        tokio::pin!(ready);
        assert!(tokio::time::timeout(Duration::from_millis(30), &mut ready).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 0, "model request has not returned a tool");
        release_first.send(()).unwrap();
        let held = ready.await.unwrap();
        assert_eq!(held.preview().remaining_steps.get(), 2);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let prefix = log.prefix(100, 128 * 1024).await.unwrap();
        assert!(prefix.events.iter().any(|event| matches!(event.event.fact, Fact::ToolSettled { .. })));
        assert_eq!(prefix.project_invocation("invocation-first").terminal, None);
        drop(held);
        second.await.unwrap();
        let next = gate.reserve(intent("committed")).unwrap();
        release_second.send(()).unwrap();
        let held = next.ready().await.unwrap();
        assert_eq!(held.preview().remaining_steps.get(), 1);
        let expected = held.preview().clone();
        let receipt = held.commit().unwrap().wait().await.unwrap();
        assert_eq!(receipt, expected);
        running.wait().await.unwrap();
        engine.drain().await;
        assert_eq!(count.load(Ordering::SeqCst), 1, "rejected calls, rollback and seal never repeat effects");
        assert!(tokio::time::timeout(Duration::from_millis(30), &mut third).await.is_err(),
            "no model request follows the committed hold");
        let before = log.prefix(100, 128 * 1024).await.unwrap();
        assert_eq!(before.events.iter().filter(|e| matches!(e.event.fact, Fact::InvocationOpened { .. })).count(), 1);
        assert!(matches!(&before.events.last().unwrap().event.fact,
            Fact::InvocationEnded { outcome: InvocationOutcome::HandoffPaused { pause } } if pause == &expected));
        let effect = Arc::new(Effect { log: log.clone(), count: count.clone() });
        assert!(engine.start(input(&base, "unrelated", effect), CancellationToken::new()).await.is_err(),
            "physical sealing does not release the logical Turn to a different admission");
        assert_eq!(log.prefix(100, 128 * 1024).await.unwrap().digest, before.digest);
        log.shutdown().await.unwrap();
        drop(engine);
        drop(log);
        let reopened = Arc::new(EventLog::open(&path).await.unwrap());
        assert_eq!(maka_agent::recovery::recover(&reopened).await.unwrap(), 0);
        let after = reopened.prefix(100, 128 * 1024).await.unwrap();
        assert_eq!(before.digest, after.digest);
        assert_eq!(after.project_invocation("invocation-first").terminal, Some(TerminalStatus::Paused));
        let prefix = reopened.run_prefix("session", "run-first", None, 100, 128 * 1024).await.unwrap().unwrap();
        let source = maka_runtime::continuation::RunBoundary {
            invocation: prefix.invocation,
            high_water: prefix.high_water,
            digest: prefix.digest,
        };
        let resumed_engine = Engine::new(reopened.clone(), ModelExecutor::new(1, Duration::from_secs(5)).unwrap(),
            CodeExecutor::new(1, CellLimits::default()).unwrap());
        let successor = || {
            let effect = Arc::new(Effect { log: reopened.clone(), count: count.clone() });
            let mut run = input(&base, "successor", effect);
            let maka_agent::RunWork::Message { tools, .. } = run.work else { unreachable!() };
            run.invocation = expected.intent.successor(&source.invocation);
            run.configuration = configuration.clone();
            run.main_output_limit = expected.execution.main_output_limit;
            run.work = maka_agent::RunWork::Handoff {
                source: source.clone(), pause: Box::new(expected.clone()), tools,
            };
            run
        };
        for field in 0..5 {
            let mut changed = successor();
            match field {
                0 => changed.main_output_limit = None,
                1 => changed.provider_options = serde_json::json!({"changed": true}),
                2 => changed.provider.model = "changed".into(),
                3 => changed.configuration.cwd.push_str("/changed"),
                _ => {
                    let maka_agent::RunWork::Handoff { tools, .. } = &mut changed.work else { unreachable!() };
                    *tools = Default::default();
                },
            }
            assert!(resumed_engine.start(changed, CancellationToken::new()).await.is_err());
            assert_eq!(reopened.prefix(100, 128 * 1024).await.unwrap().digest, after.digest);
        }
        resumed_engine.run(successor(), CancellationToken::new()).await.unwrap();
        resumed_engine.drain().await;
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        let messages = requests[2]["messages"].as_array().unwrap();
        assert_eq!(messages.iter().filter(|m| m["role"] == "user").count(), 1);
        assert_eq!(messages.iter().filter(|m| m["role"] == "tool").count(), 2);
        assert_eq!(count.load(Ordering::SeqCst), 1, "settled effects are replayed as facts, never executed");
        let finished = reopened.prefix(100, 128 * 1024).await.unwrap();
        assert_eq!(finished.project_invocation("invocation-committed").terminal, Some(TerminalStatus::Completed));
        assert!(!serde_json::to_string(&finished.events).unwrap().contains("do-not-persist-this-secret"));
        reopened.shutdown().await.unwrap();
    }).await.unwrap();
}
