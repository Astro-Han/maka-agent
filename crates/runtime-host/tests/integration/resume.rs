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

use super::support::client_probe::ClientFixture;
use maka_runtime::{event::Fact, input::InvocationInput};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_client_resumes_selected_lineage_streams_and_reopens_exact_admission() {
    let fixture = ClientFixture::new("maka-resume-");
    let mut original = None;
    for reopened in [false, true] {
        fixture
            .run(
                "--resume-workspace",
                reopened,
                if reopened {
                    "resume-reopened"
                } else {
                    "resume-passed"
                },
            )
            .await;
        let log = fixture.log().await;
        let prefix = log.prefix(500, 4 * 1024 * 1024).await.unwrap();
        assert_eq!(
            prefix
                .events
                .iter()
                .filter(|s| matches!(s.event.fact, Fact::InvocationOpened { .. }))
                .count(),
            3
        );
        assert_eq!(
            prefix
                .events
                .iter()
                .filter(|s| matches!(
                    s.event.fact,
                    Fact::InvocationOpened {
                        input: InvocationInput::Continuation { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
        let facts = serde_json::to_value(&prefix.events).unwrap();
        for stored in &prefix.events {
            if let Fact::InvocationOpened {
                configuration: Some(configuration),
                ..
            } = &stored.event.fact
            {
                configuration
                    .tool_composition
                    .as_ref()
                    .expect("Message and manual resume must freeze admitted Host handlers");
            }
        }
        if let Some(original) = &original {
            assert_eq!(&facts, original);
        } else {
            original = Some(facts);
        }
        log.close().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_resume_replays_admission_after_lost_reply_and_restart() {
    tokio::time::timeout(std::time::Duration::from_secs(30), public_resume())
        .await
        .unwrap();
}

async fn public_resume() {
    use super::support::{
        message_recovery::{Provider, configure},
        peer::Peer,
    };
    use maka_plugins::{
        composition::Scope,
        execution::{CommandError, Progress, Resume, Submit},
        fiber::Fiber,
    };
    use maka_runtime_host::server::{Host, local::LocalListener};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    let fixture = ClientFixture::new("maka-public-resume-");
    let provider = Provider::start().await;
    let model = configure(&fixture, &provider.base_url).await;
    let mut saved: Option<(Resume, maka_runtime::event::Invocation)> = None;
    for reopened in [false, true] {
        let host = Host::open(fixture.owner()).await.unwrap();
        #[cfg(unix)]
        let endpoint = fixture.workspace.parent().unwrap().join("resume.sock");
        #[cfg(windows)]
        let endpoint =
            std::path::PathBuf::from(format!(r"\\.\pipe\maka-resume-{}", uuid::Uuid::new_v4()));
        let stop = CancellationToken::new();
        let cleanup = stop.clone().drop_guard();
        let server = tokio::spawn(
            LocalListener::bind(&endpoint)
                .unwrap()
                .serve(host.clone(), stop.clone()),
        );
        let mut peer = Peer::new(host.clone(), "public-resume").await;
        if !reopened {
            let response = peer.rpc("session.create", json!({
                "sessionId":"public-resume", "workspace":{"kind":"host_path","path":fixture.workspace},
                "modelTarget":{"kind":"explicit","connectionId":model.connection_id,
                    "connectionSlug":model.connection_slug,"model":model.model}
            })).await;
            assert_eq!(response["ok"], true, "{response}");
        }
        let fiber = Fiber::new("resume-consumer", "resume-consumer", Scope::Profile).unwrap();
        fiber.begin_loading().unwrap();
        fiber.ready().unwrap();
        fiber.publish().unwrap();
        let commands = host
            .authorize_plugin_execution(fiber.context(), &["public-resume".into()])
            .await
            .unwrap();
        let (request, resumed) = if let Some((request, resumed)) = &saved {
            assert_eq!(commands.resume(request.clone()).await.unwrap(), *resumed);
            (request.clone(), resumed.clone())
        } else {
            let source = commands
                .submit(Submit {
                    operation_id: "source".into(),
                    session_id: "public-resume".into(),
                    content: "Resume this precise lineage".into(),
                    orchestration_mode: None,
                })
                .await
                .unwrap();
            loop {
                let current = commands.activity("public-resume".into()).await.unwrap();
                if !current.busy
                    && current
                        .execution
                        .as_ref()
                        .is_some_and(|run| matches!(run.progress, Progress::Ended { .. }))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            let request = Resume {
                operation_id: "continuation".into(),
                source: source.invocation,
            };
            // Dropping the observation must not detach Host's admission owner.
            let mut waiter = Box::pin(commands.resume(request.clone()));
            assert!(futures_util::FutureExt::now_or_never(waiter.as_mut()).is_none());
            drop(waiter);
            let resumed = loop {
                match commands.resume(request.clone()).await {
                    Ok(invocation) => break invocation,
                    Err(CommandError::Busy) => tokio::task::yield_now().await,
                    result => panic!("resume failed: {result:?}"),
                }
            };
            assert_ne!(resumed, request.source);
            (request, resumed)
        };
        let mut wrong = request.clone();
        wrong.source.invocation_id = "another-owner".into();
        assert!(matches!(
            commands.resume(wrong).await,
            Err(CommandError::Conflict)
        ));
        loop {
            let current = commands.activity("public-resume".into()).await.unwrap();
            if !current.busy {
                let execution = current.execution.unwrap();
                assert_eq!(execution.invocation, resumed);
                assert!(matches!(execution.progress, Progress::Ended { .. }));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(commands.resume(request.clone()).await.unwrap(), resumed);
        saved = Some((request, resumed));
        fiber
            .shutdown(tokio::time::Instant::now() + std::time::Duration::from_secs(1))
            .await
            .unwrap();
        assert!(matches!(
            commands.resume(saved.as_ref().unwrap().0.clone()).await,
            Err(CommandError::Revoked)
        ));
        peer.close().await;
        stop.cancel();
        server.await.unwrap().unwrap();
        cleanup.disarm();
    }
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        2,
        "retry and restart must not execute again"
    );
}
