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

use futures_util::future::BoxFuture;
use maka_plugins::{
    composition::Scope,
    contributions::{Catalog, Staged},
    executor::{
        Binding, Capabilities, Context, Error, Executor, Outcome, OutputSink, Provider, Request,
    },
    fiber::Fiber,
};
use maka_runtime::{event::Invocation, executor::Output};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

struct Adapter {
    started: Arc<Semaphore>,
    cooperative: bool,
}

struct PendingAdmission(Arc<Semaphore>);
impl maka_plugins::call::Admission for PendingAdmission {
    fn authorize<'a>(
        &'a self,
        _: &'a maka_plugins::call::Identity,
    ) -> BoxFuture<'a, Result<maka_plugins::call::Evidence, maka_runtime::tools::ToolError>> {
        Box::pin(async move {
            self.0.add_permits(1);
            std::future::pending().await
        })
    }
}
impl Provider for Adapter {
    fn execute(&self, _: Request, context: Context) -> BoxFuture<'static, Result<Outcome, Error>> {
        let started = self.started.clone();
        let cooperative = self.cooperative;
        Box::pin(async move {
            let scope = context.call.as_ref().expect("embedding-issued call scope");
            assert_eq!(scope.identity.agent().unwrap().session_id, "session");
            let mut resource = scope.resources.reserve().unwrap();
            resource.start();
            context
                .output
                .emit(Output::OutputDelta {
                    text: "accepted external text".into(),
                })
                .await?;
            started.add_permits(1);
            if !cooperative {
                std::future::pending::<()>().await;
            }
            context.cancellation.cancelled().await;
            assert!(scope.cancellation.is_cancelled());
            resource.complete(Ok(()));
            assert!(matches!(
                context
                    .output
                    .emit(Output::OutputDelta {
                        text: "late".into()
                    })
                    .await,
                Err(Error::Cancelled)
            ));
            Ok(Outcome::Completed {
                text: "must not mask cancellation".into(),
            })
        })
    }
}
#[derive(Default)]
struct Sink(Mutex<Vec<Output>>);
impl OutputSink for Sink {
    fn emit(&self, output: Output) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(output);
            Ok(())
        })
    }
}

#[tokio::test]
async fn retirement_cancels_exact_executor_and_keeps_settlement_lease_or_fences_failed_cleanup() {
    for (admitting, cooperative) in [(true, true), (false, true), (false, false)] {
        let catalog = Catalog::default();
        let fiber = Fiber::new("executor", "executor", Scope::Profile).unwrap();
        fiber.begin_loading().unwrap();
        fiber.ready().unwrap();
        fiber.publish().unwrap();
        let started = Arc::new(Semaphore::new(0));
        let mut staged = Staged::default();
        staged
            .insert(
                "example",
                Executor {
                    id: "example".to_owned().try_into().unwrap(),
                    display_name: "Example".into(),
                    capabilities: Capabilities::default(),
                    provider: Arc::new(Adapter {
                        started: started.clone(),
                        cooperative,
                    }),
                },
            )
            .unwrap();
        let registration = catalog.register(&fiber.context(), staged).unwrap();
        let contribution = catalog
            .snapshot::<Executor>(&Scope::Session("session".into()))
            .entries
            .remove("example")
            .unwrap();
        let issuer = if admitting {
            maka_plugins::call::Issuer::with_admission(Arc::new(PendingAdmission(started.clone())))
        } else {
            maka_plugins::call::Issuer::default()
        };
        let binding = Binding::new("session".into(), contribution)
            .unwrap()
            .with_calls(issuer);
        let request = Request {
            invocation: Invocation {
                session_id: "session".into(),
                turn_id: "turn".into(),
                run_id: "run".into(),
                invocation_id: "invocation".into(),
            },
            conversation_key: "session".into(),
            settings: Default::default(),
            content: "work".into(),
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            instructions: None,
        };
        let call = binding.admit(request.clone()).unwrap();
        let sink = Arc::new(Sink::default());
        let run = tokio::spawn(call.execute(sink.clone(), CancellationToken::new()));
        started.acquire().await.unwrap().forget();
        drop(registration);
        assert!(matches!(binding.admit(request), Err(Error::Retired)));
        let settlement = tokio::time::timeout(Duration::from_secs(7), run)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            fiber.context().active_calls(),
            1,
            "Host terminal commit still owns the lease"
        );
        assert_eq!(sink.0.lock().unwrap().len(), usize::from(!admitting));
        if cooperative {
            assert!(matches!(settlement.result, Err(Error::Retired)));
            assert!(
                fiber.context().is_effective(),
                "revoking one contribution does not retire its owner"
            );
        } else {
            assert!(matches!(settlement.result, Err(Error::CleanupUnconfirmed)));
            assert!(!fiber.context().is_effective());
        }
        drop(settlement);
        let closed = fiber
            .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
            .await;
        if cooperative {
            closed.unwrap();
        } else {
            assert!(matches!(closed, Err(maka_plugins::Error::Cleanup(_))));
        }
    }
}
