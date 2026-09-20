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

use maka_client_capability::{
    Endpoint, Identity, PrincipalKind, Registry,
    broker::{Broker, CallError, ToolCall},
};
use maka_event_log::EventLog;
use maka_runtime::{
    capability::{AdmissionEvidence, CallResult, ClientFrame, HostFrame},
    event::{EventWrite, Fact, Invocation, InvocationInput, RuntimeEvent, ToolOutcome},
    tool_call::ToolCallIdentity,
    tools::{ToolError, ToolJournal},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[path = "support/journal_sink.rs"]
mod journal_sink;
use journal_sink::{Boundary, Sink};

#[tokio::test]
async fn prepared_remote_admission_obeys_real_log_cuts_and_retains_unknown_outcomes() {
    for boundary in [
        Boundary::Success,
        Boundary::RejectT1,
        Boundary::UnknownT1,
        Boundary::CancelAfterT1,
        Boundary::LostAfterT1,
        Boundary::LostAfterEffect,
        Boundary::UnknownT2,
    ] {
        tokio::time::timeout(Duration::from_secs(5), run(boundary))
            .await
            .unwrap_or_else(|_| panic!("journal admission timed out: {boundary:?}"));
    }
}

async fn run(boundary: Boundary) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("events.sqlite");
    let log = Arc::new(EventLog::open(&path).await.unwrap());
    let invocation = Invocation {
        session_id: "session".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    log.append(
        &EventWrite::plain(RuntimeEvent::new(
            invocation.clone(),
            Fact::InvocationOpened {
                configuration: None,
                input: InvocationInput::Message {
                    source_messages: Vec::new(),
                    content: "invoke".into(),
                    request_fingerprint: None,
                },
            },
        ))
        .unwrap(),
    )
    .await
    .unwrap();
    let mut registry = Registry::default();
    let connection = Uuid::new_v4();
    let (endpoint, mut outbound) = Endpoint::channel(8);
    let provider = registry
        .attach(
            connection,
            Identity {
                principal_kind: PrincipalKind::LocalOwner,
                principal_id: "owner".into(),
                client_instance_id: "client".into(),
                credential_bound_client_instance_id: None,
                capability_owner: None,
            },
            endpoint.clone(),
        )
        .unwrap();
    registry
        .replace(
            connection,
            maka_protocol::capability::decode_replace_input(&json!({
                "registrationId":"r",
                "offers":[{"offerId":"offer","version":"1","affinity":"session",
                    "hostPathAccess":"none","label":"Remote",
                    "tools":[{"serverId":"server","name":"effect","inputSchema":{"type":"object"}}]}]
            })).unwrap(),
        )
        .unwrap();
    let broker = Broker::default();
    let cancellation = CancellationToken::new();
    let pending = broker
        .prepare_tool(
            registry.current(&provider).unwrap(),
            ToolCall {
                offer_id: "offer".into(),
                server_id: "server".into(),
                tool_name: "effect".into(),
                arguments: serde_json::Map::new(),
                source: maka_runtime::capability::CallSource::Agent {
                    session_id: invocation.session_id.clone(),
                    turn_id: invocation.turn_id.clone(),
                },
                tool_call_id: "call".into(),
                cwd: "/host/private/workspace".into(),
            },
            Duration::from_secs(2),
            cancellation.clone(),
        )
        .unwrap();
    let id = pending.invocation_id().to_owned();
    assert!(matches!(
        outbound.recv().await.unwrap(),
        HostFrame::Call { cwd: None, .. }
    ));
    broker
        .accept(
            connection,
            ClientFrame::Accepted {
                invocation_id: id.clone(),
                admission_evidence: AdmissionEvidence::None,
            },
        )
        .unwrap();
    let accepted = pending.accepted().await.unwrap();
    let session = invocation.session_id.clone();
    let turn = invocation.turn_id.clone();
    let admitted = || broker.admitted_tool(connection, &session, &turn, "call", "server", "effect");
    assert!(!admitted(), "acceptance is not permission for callbacks");
    assert_eq!(
        log.prefix(8, 16384).await.unwrap().events.len(),
        1,
        "provider acceptance adds no execution fact"
    );
    let journal = ToolJournal::new(
        Arc::new(Sink {
            log: log.clone(),
            boundary,
            cancellation: cancellation.clone(),
            endpoint: endpoint.clone(),
        }),
        invocation,
    );
    let execution = tokio::spawn(journal.invoke_call_with(
        "operation".into(),
        ToolCallIdentity::standalone("call".into()),
        "remote".into(),
        json!({}),
        cancellation,
        move |_| {
            Box::pin(async move {
                let result = accepted.admit().await.map_err(|error| match error {
                    CallError::OutcomeUnknown(_) => ToolError::OutcomeUnknown(error.to_string()),
                    _ => ToolError::Failed(error.to_string()),
                })?;
                serde_json::to_value(result).map_err(|e| ToolError::OutcomeUnknown(e.to_string()))
            })
        },
    ));
    let should_execute = matches!(
        boundary,
        Boundary::Success | Boundary::UnknownT2 | Boundary::LostAfterEffect
    );
    let effect_path = directory.path().join("effect");
    if should_execute {
        assert!(
            matches!(outbound.recv().await.unwrap(),HostFrame::Admitted{invocation_id} if invocation_id==id)
        );
        registry.unregister(connection, "r").unwrap();
        assert!(admitted(), "callbacks retain the admitted publication");
        assert!(!broker.admitted_tool(Uuid::new_v4(), &session, &turn, "call", "server", "effect"));
        assert!(!broker.admitted_tool(
            connection,
            &session,
            "other-turn",
            "call",
            "server",
            "effect"
        ));
        assert!(!broker.admitted_tool(
            connection,
            &session,
            &turn,
            "other-call",
            "server",
            "effect"
        ));
        let prefix = log.prefix(8, 16384).await.unwrap();
        assert!(
            matches!(&prefix.events.last().unwrap().event.fact,Fact::ToolDispatched{operation_id,..} if operation_id=="operation")
        );
        std::fs::write(&effect_path, "once").unwrap();
        if boundary == Boundary::LostAfterEffect {
            endpoint.close();
        } else {
            broker
                .accept(
                    connection,
                    ClientFrame::Result {
                        invocation_id: id.clone(),
                        result: CallResult {
                            content: vec![],
                            structured_content: Some(json!({"ok":true})),
                        },
                    },
                )
                .unwrap();
        }
    }
    let result = execution.await.unwrap();
    assert!(!admitted(), "settlement or cancellation revokes callbacks");
    match boundary {
        Boundary::Success => {
            assert!(result.is_ok());
        }
        Boundary::RejectT1 | Boundary::UnknownT1 => {
            assert!(matches!(result, Err(ToolError::Persistence(_))))
        }
        Boundary::CancelAfterT1 | Boundary::LostAfterT1 => {
            assert!(matches!(result, Err(ToolError::Failed(_))))
        }
        Boundary::UnknownT2 | Boundary::LostAfterEffect => {
            assert!(matches!(result, Err(ToolError::OutcomeUnknown(_))))
        }
    }
    assert_eq!(effect_path.exists(), should_execute);
    while let Ok(frame) = outbound.try_recv() {
        assert!(
            !matches!(frame, HostFrame::Admitted { .. }),
            "failed preparation leaked an admission"
        );
    }
    let before = log.prefix(8, 16384).await.unwrap();
    let uncertain = before.project_invocation("invocation").uncertain_operations;
    assert_eq!(
        !uncertain.is_empty(),
        matches!(
            boundary,
            Boundary::UnknownT1 | Boundary::UnknownT2 | Boundary::LostAfterEffect
        )
    );
    match boundary {
        Boundary::RejectT1 => assert_eq!(before.events.len(), 1),
        Boundary::UnknownT1 | Boundary::UnknownT2 | Boundary::LostAfterEffect => {
            assert_eq!(before.events.len(), 2)
        }
        Boundary::Success => assert!(matches!(
            &before.events.last().unwrap().event.fact,
            Fact::ToolSettled {
                outcome: ToolOutcome::Succeeded { .. },
                ..
            }
        )),
        Boundary::CancelAfterT1 | Boundary::LostAfterT1 => assert!(matches!(
            &before.events.last().unwrap().event.fact,
            Fact::ToolSettled {
                outcome: ToolOutcome::Failed { .. },
                ..
            }
        )),
    }
    broker.shutdown().await;
    registry.begin_drain();
    drop(journal);
    log.shutdown().await.unwrap();
    let reopened = EventLog::open(&path).await.unwrap();
    let after = reopened.prefix(8, 16384).await.unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    assert_eq!(effect_path.exists(), should_execute);
    reopened.close().await.unwrap();
}
