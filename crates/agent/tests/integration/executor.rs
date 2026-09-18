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

use super::support::{context, invocation};
use futures_util::future::BoxFuture;
use maka_agent::{ExecutorInput, RunError};
use maka_event_log::EventLog;
use maka_plugins::{
    composition::Scope,
    contributions::{Catalog, Staged},
    executor::{Binding, Capabilities, Context, Error, Executor, Outcome, Provider, Request},
    fiber::Fiber,
};
use maka_runtime::{
    event::{Fact, Invocation, InvocationOutcome},
    execution::ToolMode,
    executor::Output,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

struct Adapter {
    started: Arc<Semaphore>,
    retained: Arc<Mutex<Option<Context>>>,
}
impl Provider for Adapter {
    fn execute(
        &self,
        request: Request,
        context: Context,
    ) -> BoxFuture<'static, Result<Outcome, Error>> {
        let started = self.started.clone();
        let retained = self.retained.clone();
        Box::pin(async move {
            context
                .output
                .emit(Output::ThinkingDelta {
                    text: "thought".into(),
                })
                .await?;
            context
                .output
                .emit(Output::OutputDelta {
                    text: "partial".into(),
                })
                .await?;
            *retained.lock().unwrap() = Some(context.clone());
            started.add_permits(1);
            if request.content.text == "wait" {
                context.cancellation.cancelled().await;
            }
            Ok(Outcome::Completed {
                text: "answer".into(),
            })
        })
    }
}

#[tokio::test]
async fn external_backend_uses_canonical_admission_settlement_and_never_enters_model_loop() {
    let temp = tempfile::tempdir().unwrap();
    let log = Arc::new(
        EventLog::open(&temp.path().join("executor.sqlite"))
            .await
            .unwrap(),
    );
    let engine = context::engine(log.clone());
    let catalog = Catalog::default();
    let fiber = Fiber::new("example", "example", Scope::Profile).unwrap();
    fiber.begin_loading().unwrap();
    fiber.ready().unwrap();
    fiber.publish().unwrap();
    let started = Arc::new(Semaphore::new(0));
    let retained = Arc::new(Mutex::new(None));
    let mut staged = Staged::default();
    staged
        .insert(
            "example",
            Executor {
                id: "example".to_owned().try_into().unwrap(),
                display_name: "Example".into(),
                capabilities: Capabilities {
                    thinking: true,
                    ..Default::default()
                },
                provider: Arc::new(Adapter {
                    started: started.clone(),
                    retained: retained.clone(),
                }),
            },
        )
        .unwrap();
    let registration = catalog.register(&fiber.context(), staged).unwrap();
    for id in ["complete", "wait"] {
        let mut configuration = invocation::configuration(ToolMode::Direct);
        configuration.model = None;
        let contribution = catalog
            .snapshot::<Executor>(&Scope::Profile)
            .entries
            .remove("example")
            .unwrap();
        let input = ExecutorInput {
            request: Request {
                invocation: Invocation {
                    session_id: "session".into(),
                    turn_id: id.into(),
                    run_id: id.into(),
                    invocation_id: id.into(),
                },
                conversation_key: "session".into(),
                content: id.into(),
                cwd: configuration.cwd.clone(),
                instructions: None,
            },
            binding: Binding::new("session".into(), contribution).unwrap(),
            configuration,
            request_fingerprint: Some(id.into()),
            source_messages: vec![],
        };
        let running = engine
            .start_executor(input, CancellationToken::new())
            .await
            .unwrap();
        assert!(running.handoff().is_none());
        started.acquire().await.unwrap().forget();
        if id == "wait" {
            let call = running.cancellation();
            call.cancel();
        }
        let result = running.wait().await;
        if id == "complete" {
            result.unwrap();
        } else {
            assert!(matches!(result, Err(RunError::Cancelled)));
        }
        let old = retained.lock().unwrap().take().unwrap();
        assert!(matches!(
            old.output
                .emit(Output::OutputDelta {
                    text: "late".into()
                })
                .await,
            Err(Error::Cancelled)
        ));
    }
    drop(registration);
    fiber
        .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
        .await
        .unwrap();
    engine.drain().await;
    let prefix = log.prefix(100, 1024 * 1024).await.unwrap();
    assert_eq!(
        prefix
            .events
            .iter()
            .filter(|row| matches!(row.event.fact, Fact::ExecutorCompleted { .. }))
            .count(),
        1
    );
    assert!(
        prefix
            .events
            .iter()
            .any(|row| row.event.invocation.turn_id == "wait"
                && matches!(
                    row.event.fact,
                    Fact::InvocationEnded {
                        outcome: InvocationOutcome::Cancelled { .. }
                    }
                ))
    );
    assert!(!prefix.events.iter().any(|row| matches!(
        row.event.fact,
        Fact::ModelRequested { .. } | Fact::ToolDispatched { .. }
    )));
    assert!(
        log.prepare_transcript("session", prefix.high_water, 32)
            .await
            .unwrap()
    );
    let stream = log
        .session_stream_events("session", 0, prefix.high_water, 64, 1024 * 1024)
        .await
        .unwrap();
    assert!(stream.events.iter().any(|event| event.invocation.turn_id == "wait"
        && matches!(&event.fact, maka_event_log::observation::StreamFact::InvocationEnded { interrupted, .. } if interrupted.len() == 1)));
    // A crash after the external dispatch cannot be downgraded to "never ran".
    // Recovery seals it without invoking the adapter or claiming cleanup.
    let crashed = Invocation {
        turn_id: "crashed".into(),
        run_id: "crashed".into(),
        invocation_id: "crashed".into(),
        session_id: "session".into(),
    };
    for stored in prefix
        .events
        .iter()
        .filter(|row| row.event.invocation.invocation_id == "complete")
    {
        if matches!(
            stored.event.fact,
            Fact::InvocationOpened { .. }
                | Fact::ExecutorStarted { .. }
                | Fact::ExecutorObserved { .. }
        ) {
            log.append(
                &maka_runtime::event::EventWrite::plain(maka_runtime::event::RuntimeEvent::new(
                    crashed.clone(),
                    stored.event.fact.clone(),
                ))
                .unwrap(),
            )
            .await
            .unwrap();
        }
    }
    assert!(
        log.invocation_recovery(&crashed, 32, 1024 * 1024)
            .await
            .unwrap()
            .unfinished_executor
    );
    assert_eq!(maka_agent::recovery::recover(&log).await.unwrap(), 1);
    assert_eq!(maka_agent::recovery::recover(&log).await.unwrap(), 0);
    let recovered = log.prefix(100, 1024 * 1024).await.unwrap();
    assert!(recovered.project_invocation("crashed").unfinished_executor);
    assert!(recovered.events.iter().any(|row| row.event.invocation == crashed
        && matches!(&row.event.fact, Fact::InvocationEnded { outcome: InvocationOutcome::Failed { class, .. } } if class == "outcome_unknown")));
    assert_eq!(
        started.available_permits(),
        0,
        "recovery must not call the adapter"
    );
    log.shutdown().await.unwrap();
}
