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

#[path = "support/forms.rs"]
mod support;
use maka_client_capability::broker::{Broker, CallError};
use maka_runtime::capability::{ClientFrame, FormResult, HostFrame};
use std::{sync::Arc, time::Duration};
use support::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn disconnect_and_drain_withdraw_before_settling() {
    for disconnect in [false, true] {
        let transport = CancellationToken::new();
        let (_registry, connection, registration, mut out) = setup(transport.clone());
        let broker = Arc::new(Broker::default());
        let (tx, mut requests) = mpsc::unbounded_channel();
        let pending = prepare(&broker, &registration).unwrap();
        let id = pending.invocation_id().to_owned();
        out.recv().await.unwrap();
        broker.accept(connection, provider_accept(&id)).unwrap();
        let result = tokio::spawn(
            pending
                .accepted()
                .await
                .unwrap()
                .admit_with_interactions(Arc::new(Handler(tx))),
        );
        out.recv().await.unwrap();
        broker.accept(connection, form(&id, "provider")).unwrap();
        let request = requests.recv().await.unwrap();
        if disconnect {
            transport.cancel();
        }
        let shutdown = tokio::spawn({
            let broker = broker.clone();
            async move { broker.shutdown().await }
        });
        request.cancellation.cancelled().await;
        assert!(!shutdown.is_finished());
        assert!(!result.is_finished());
        request.withdrawn.send(()).unwrap();
        shutdown.await.unwrap();
        assert!(matches!(
            result.await.unwrap(),
            Err(CallError::OutcomeUnknown(_))
        ));
        while let Ok(frame) = out.try_recv() {
            assert!(!matches!(frame, HostFrame::InteractionResult { .. }));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn forms_pause_deadline_and_allow_immediate_next_form_and_result() {
    let (_registry, connection, registration, mut out) = setup(CancellationToken::new());
    let broker = Broker::default();
    let (tx, mut requests) = mpsc::unbounded_channel();
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let result = tokio::spawn(
        pending
            .accepted()
            .await
            .unwrap()
            .admit_with_interactions(Arc::new(Handler(tx))),
    );
    is_control(out.recv().await.unwrap(), "admitted", &id);
    tokio::time::advance(Duration::from_secs(3)).await;
    broker.accept(connection, form(&id, "provider-1")).unwrap();
    let request = requests.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(3600)).await;
    assert!(!result.is_finished());
    assert!(broker.accept(connection, final_result(&id)).is_err());
    assert!(
        broker
            .accept(
                connection,
                ClientFrame::ResultStart {
                    invocation_id: id.clone(),
                    byte_length: 1,
                    chunk_count: 1
                }
            )
            .is_err()
    );
    assert!(broker.accept(connection, form(&id, "overlap")).is_err());
    request.answer.send(FormResult::Decline).unwrap();
    assert!(
        matches!(out.recv().await.unwrap(), HostFrame::InteractionResult { interaction_id, result: FormResult::Decline, .. } if interaction_id == "provider-1")
    );
    broker.accept(connection, form(&id, "provider-2")).unwrap();
    requests
        .recv()
        .await
        .unwrap()
        .answer
        .send(FormResult::Cancel)
        .unwrap();
    assert!(
        matches!(out.recv().await.unwrap(), HostFrame::InteractionResult { interaction_id, .. } if interaction_id == "provider-2")
    );
    broker.accept(connection, final_result(&id)).unwrap();
    assert!(result.await.unwrap().is_ok());
    is_control(out.recv().await.unwrap(), "release", &id);
    broker.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn resumed_execution_gets_a_fresh_timeout() {
    let (_registry, connection, registration, mut out) = setup(CancellationToken::new());
    let broker = Broker::default();
    let (tx, mut requests) = mpsc::unbounded_channel();
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let result = tokio::spawn(
        pending
            .accepted()
            .await
            .unwrap()
            .admit_with_interactions(Arc::new(Handler(tx))),
    );
    out.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(4)).await;
    broker.accept(connection, form(&id, "provider")).unwrap();
    let request = requests.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(3600)).await;
    request.answer.send(FormResult::Decline).unwrap();
    out.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(!result.is_finished());
    tokio::time::advance(Duration::from_secs(4)).await;
    assert!(matches!(
        result.await.unwrap(),
        Err(CallError::OutcomeUnknown(_))
    ));
    is_control(out.recv().await.unwrap(), "cancel", &id);
    is_control(out.recv().await.unwrap(), "release", &id);
    broker.shutdown().await;
}

#[tokio::test]
async fn failed_provider_preserves_known_outcome_until_withdrawal_finishes() {
    let (_registry, connection, registration, mut out) = setup(CancellationToken::new());
    let broker = Arc::new(Broker::default());
    let (tx, mut requests) = mpsc::unbounded_channel();
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let result = tokio::spawn(
        pending
            .accepted()
            .await
            .unwrap()
            .admit_with_interactions(Arc::new(Handler(tx))),
    );
    out.recv().await.unwrap();
    broker.accept(connection, form(&id, "provider")).unwrap();
    let request = requests.recv().await.unwrap();
    broker
        .accept(
            connection,
            ClientFrame::Failed {
                invocation_id: id.clone(),
                message: "known failure".into(),
            },
        )
        .unwrap();
    request.cancellation.cancelled().await;
    assert!(!result.is_finished());
    assert!(out.try_recv().is_err());
    broker.accept(connection, final_result(&id)).unwrap();
    let shutdown = tokio::spawn({
        let broker = broker.clone();
        async move { broker.shutdown().await }
    });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished());
    request.withdrawn.send(()).unwrap();
    assert_eq!(
        result.await.unwrap().unwrap_err(),
        CallError::ProviderFailed("known failure".into())
    );
    is_control(out.recv().await.unwrap(), "release", &id);
    shutdown.await.unwrap();
}

#[tokio::test]
async fn absent_handler_cancels_with_unknown_outcome() {
    let (_registry, connection, registration, mut out) = setup(CancellationToken::new());
    let broker = Broker::default();
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let result = tokio::spawn(pending.accepted().await.unwrap().admit());
    out.recv().await.unwrap();
    broker.accept(connection, form(&id, "provider")).unwrap();
    assert!(matches!(
        result.await.unwrap(),
        Err(CallError::OutcomeUnknown(_))
    ));
    is_control(out.recv().await.unwrap(), "cancel", &id);
    is_control(out.recv().await.unwrap(), "release", &id);
    broker.shutdown().await;
}

#[tokio::test]
async fn dropped_waiter_retains_capacity_and_shutdown_waits_for_withdrawal() {
    let (_registry, connection, registration, mut out) = setup(CancellationToken::new());
    let broker = Arc::new(Broker::default());
    let (tx, mut requests) = mpsc::unbounded_channel();
    let pending = prepare(&broker, &registration).unwrap();
    let id = pending.invocation_id().to_owned();
    out.recv().await.unwrap();
    broker.accept(connection, provider_accept(&id)).unwrap();
    let result = tokio::spawn(
        pending
            .accepted()
            .await
            .unwrap()
            .admit_with_interactions(Arc::new(Handler(tx))),
    );
    out.recv().await.unwrap();
    broker.accept(connection, form(&id, "provider")).unwrap();
    let request = requests.recv().await.unwrap();
    let mut held = vec![];
    for _ in 0..7 {
        held.push(prepare(&broker, &registration).unwrap());
        out.recv().await.unwrap();
    }
    result.abort();
    let _ = result.await;
    request.cancellation.cancelled().await;
    is_control(out.recv().await.unwrap(), "cancel", &id);
    assert!(out.try_recv().is_err());
    assert!(matches!(
        prepare(&broker, &registration),
        Err(CallError::Overloaded)
    ));
    drop(held);
    while out.try_recv().is_ok() {}
    let shutdown = tokio::spawn({
        let broker = broker.clone();
        async move { broker.shutdown().await }
    });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished());
    request.withdrawn.send(()).unwrap();
    shutdown.await.unwrap();
    is_control(out.recv().await.unwrap(), "release", &id);
}
