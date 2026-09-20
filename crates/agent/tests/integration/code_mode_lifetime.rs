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

use crate::support::invocation;

use crate::support::code_mode_lifetime as fixture;
use crate::support::http;
use fixture::{SlowEffect, respond};

use futures_util::poll;
use maka_agent::{Engine, RunError, RunInput};
use maka_event_log::EventLog;
use maka_js_runtime::{CellLimits, CodeExecutor};
use maka_model::{ModelExecutor, ProviderConfig, ProviderKind};
use maka_runtime::event::{Fact, Invocation, InvocationOutcome, TerminalStatus, ToolOutcome};
use maka_tools::{
    ToolCatalog, ToolDefinition, ToolHandler, ToolMode, ToolNesting, ToolRegistration,
    ToolSemantics,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

fn input(base: &str, suffix: &str, effect: Arc<SlowEffect>) -> RunInput {
    RunInput {
        main_output_limit: None,
        context: None,
        invocation: Invocation {
            session_id: "session".into(),
            turn_id: format!("turn-{suffix}"),
            run_id: format!("run-{suffix}"),
            invocation_id: format!("invocation-{suffix}"),
        },
        request_fingerprint: None,
        provider: ProviderConfig {
            kind: ProviderKind::OpenaiChat,
            model: "test".into(),
            base_url: base.into(),
            auth: maka_model::ProviderAuth::ApiKey("fixture-key".into()),
            headers: BTreeMap::new(),
            network: Default::default(),
            body_overlay: None,
        },
        provider_options: json!({}),
        supports_vision: false,
        configuration: invocation::configuration(ToolMode::CodeMode),
        work: maka_agent::RunWork::Message {
            source_messages: Vec::new(),
            message: format!("question {suffix}").into(),
            tools: ToolCatalog::new([ToolRegistration {
                definition: ToolDefinition {
                    name: "slow".into(),
                    description: "fixture effect draining after cancellation".into(),
                    input_schema: json!({"type":"object"}),
                },
                nesting: ToolNesting::Nestable,
                semantics: ToolSemantics::Parallel,
                handler: ToolHandler::Immediate(effect),
            }])
            .unwrap(),
            max_steps: 1,
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_caller_keeps_unawaited_child_session_and_cell_owned_until_drain() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests_seen = Arc::new(AtomicUsize::new(0));
        let server_count = requests_seen.clone();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for index in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                requests.push(http::read_request(&mut socket).await);
                server_count.fetch_add(1, Ordering::SeqCst);
                respond(&mut socket, index == 0).await;
            }
            requests
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.sqlite");
        let effect = Arc::new(SlowEffect::default());
        let committed;
        {
            let log = Arc::new(EventLog::open(&path).await.unwrap());
            let cells = CodeExecutor::new(1, CellLimits::default()).unwrap();
            let engine = Engine::new(
                log.clone(),
                ModelExecutor::new(1, Duration::from_secs(10)).unwrap(),
                cells.clone(),
            );
            let owner = engine.clone();
            let running_input = input(&base, "aborted", effect.clone());
            let caller_token = CancellationToken::new();
            let token = caller_token.clone();
            let caller = tokio::spawn(async move { owner.run(running_input, token).await });
            effect.entered.notified().await;
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
            effect.cancelled.notified().await;
            assert!(!caller_token.is_cancelled());
            let held = log.prefix(100, 128 * 1024).await.unwrap();
            assert_eq!(held.project_invocation("invocation-aborted").terminal, None);
            assert_eq!(
                held.events
                    .iter()
                    .filter(|event| matches!(event.event.fact, Fact::ToolDispatched { .. }))
                    .count(),
                2
            );
            assert!(!held.events.iter().any(|event| matches!(
                event.event.fact,
                Fact::ToolSettled { .. } | Fact::InvocationEnded { .. }
            )));
            assert!(matches!(
                engine
                    .run(
                        input(&base, "blocked", effect.clone()),
                        CancellationToken::new()
                    )
                    .await,
                Err(RunError::Busy)
            ));
            let next = cells.execute("return 7;".into(), effect.clone(), CancellationToken::new());
            tokio::pin!(next);
            // Explicitly poll the contender while the cancelled effect remains
            // blocked on release. Cancellation must not free the shared permit.
            assert!(poll!(&mut next).is_pending());
            assert_eq!(
                log.prefix(100, 128 * 1024).await.unwrap().high_water,
                held.high_water
            );
            assert_eq!(requests_seen.load(Ordering::SeqCst), 1);
            effect.release.notify_one();
            assert_eq!(
                serde_json::to_value(next.await.unwrap()).unwrap(),
                json!({"ok":true,"value":7,"toolCalls":[]})
            );
            engine.drain().await;
            let settled = log.prefix(100, 128 * 1024).await.unwrap();
            let state = settled.project_invocation("invocation-aborted");
            assert_eq!(state.terminal, Some(TerminalStatus::Cancelled));
            assert!(state.uncertain_operations.is_empty());
            assert!(state.unfinished_model_steps.is_empty());
            let boundaries: Vec<_> = settled
                .events
                .iter()
                .filter(|event| !matches!(event.event.fact, Fact::ModelObserved { .. }))
                .collect();
            assert_eq!(
                boundaries
                    .iter()
                    .map(|event| event.event.fact.kind())
                    .collect::<Vec<_>>(),
                [
                    "invocation_opened",
                    "model_requested",
                    "model_completed",
                    "tool_dispatched",
                    "tool_dispatched",
                    "tool_settled",
                    "tool_settled",
                    "invocation_ended"
                ]
            );
            let Fact::ToolDispatched {
                operation_id: parent,
                name,
                ..
            } = &boundaries[3].event.fact
            else {
                panic!("parent T1")
            };
            assert_eq!(name, "exec");
            let Fact::ToolDispatched {
                operation_id: child,
                name,
                ..
            } = &boundaries[4].event.fact
            else {
                panic!("child T1")
            };
            assert_eq!(name, "slow");
            assert!(matches!(&boundaries[5].event.fact,
                Fact::ToolSettled { operation_id, outcome: ToolOutcome::Succeeded { .. } } if operation_id == child));
            assert_eq!(log.resolve_tool_result(&boundaries[5].event.invocation.session_id, &boundaries[5].event.id).await.unwrap().into_json(), json!({"value":42}));
            assert_eq!(
                boundaries[6].event.fact,
                Fact::ToolSettled {
                    operation_id: parent.clone(),
                    outcome: ToolOutcome::Failed {
                        message: "code execution cancelled".into()
                    },
                }
            );
            assert_eq!(
                boundaries[7].event.fact,
                Fact::InvocationEnded {
                    outcome: InvocationOutcome::Cancelled {
                        source: "runtime_cancellation".into()
                    },
                }
            );
            assert_eq!(
                requests_seen.load(Ordering::SeqCst),
                1,
                "cancelled invocation must not request another model step"
            );
            committed = serde_json::to_value(&settled.events).unwrap();
            drop(engine);
            Arc::try_unwrap(log).ok().unwrap().close().await.unwrap();
        }
        {
            let log = Arc::new(EventLog::open(&path).await.unwrap());
            let recovered = log.prefix(100, 128 * 1024).await.unwrap();
            assert_eq!(serde_json::to_value(&recovered.events).unwrap(), committed);
            assert!(
                recovered
                    .project_invocation("invocation-aborted")
                    .uncertain_operations
                    .is_empty()
            );
            let engine = Engine::new(
                log.clone(),
                ModelExecutor::new(1, Duration::from_secs(10)).unwrap(),
                CodeExecutor::new(1, CellLimits::default()).unwrap(),
            );
            engine
                .run(
                    input(&base, "reopened", effect.clone()),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            engine.drain().await;
            assert_eq!(
                effect.count.load(Ordering::SeqCst),
                1,
                "reopening must not replay the drained effect"
            );
            assert_eq!(
                log.prefix(200, 256 * 1024)
                    .await
                    .unwrap()
                    .project_invocation("invocation-reopened")
                    .terminal,
                Some(TerminalStatus::Completed)
            );
        }
        let requests = server.await.unwrap();
        let history = requests[1]["messages"].as_array().unwrap();
        assert_eq!(history.len(), 4);
        assert_eq!(history[2]["role"], "tool");
        assert_eq!(history[2]["tool_call_id"], "exec-slow");
        assert_eq!(history[2]["content"], "code execution cancelled");
        assert_eq!(
            history[3],
            json!({"role":"user","content":"question reopened"})
        );
    })
    .await
    .expect("cancelled unawaited Code Mode work must drain within the test bound");
}
