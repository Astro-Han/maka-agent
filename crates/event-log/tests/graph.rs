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
use maka_event_log::{EventLog, StoreError};
use maka_graph::{
    Mode, OperatorId, WorkId,
    control::{Intent, Wake},
    schedule::{Schedule, Source, Target, Update, Work},
};
use maka_plugins::{composition::Scope, execution::Submit, storage::Namespace};
use maka_runtime::{
    event::{EventWrite, Fact, Invocation, InvocationInput, RuntimeEvent},
    tool_call::ToolCallIdentity,
};
use serde_json::json;

#[tokio::test]
async fn committed_graph_decisions_and_host_receipts_recover_without_retargeting() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("graph.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    for id in ["root", "child"] {
        log.create_session(id, id, &json!({}), 1).await.unwrap();
    }
    let epoch = log.open_graph("root", Mode::Graph, None, 2).await.unwrap();
    let invocation = Invocation {
        session_id: "root".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    let update = Update {
        graph_id: epoch.graph_id.clone(),
        source: Source {
            invocation: invocation.clone(),
            operation_id: "decision".into(),
        },
        add_work: vec![Work {
            work_id: WorkId::new(),
            target: Target::Agent {
                agent_id: "general".into(),
                executor_id: None,
            },
            instruction: "Inspect the source".into(),
            input_ids: vec![],
            selected_result_inputs: vec![],
            replaces: None,
        }],
        stop: vec![],
        finish: None,
    };
    assert!(log.commit_graph_update(update.clone(), 0, 3).await.is_err());
    for fact in [
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: "Coordinate".into(),
                source_messages: vec![],
                request_fingerprint: None,
                skill_invocation: None,
            },
        },
        Fact::ToolDispatched {
            operation_id: "decision".into(),
            call: ToolCallIdentity::standalone("call".into()),
            name: "update_agent_graph".into(),
            input: json!({}),
        },
    ] {
        log.append(&EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap())
            .await
            .unwrap();
    }
    let committed = log.commit_graph_update(update.clone(), 0, 3).await.unwrap();
    let work = &update.add_work[0];
    assert_eq!(
        log.graph_work("root", &epoch.graph_id, &work.work_id)
            .await
            .unwrap()
            .as_ref(),
        Some(work)
    );
    assert!(
        log.graph_work("child", &epoch.graph_id, &work.work_id)
            .await
            .unwrap()
            .is_none()
    );
    // Lost Tool response must not erase a committed scheduling decision.
    let intent = Intent {
        graph_id: epoch.graph_id.clone(),
        work_id: update.add_work[0].work_id.clone(),
        operator_id: OperatorId::new(),
        request: Submit {
            operation_id: "work-1".into(),
            orchestration_mode: None,
            session_id: "child".into(),
            content: "Inspect the source".into(),
        },
        schedule_revision: committed.revision,
    };
    log.commit_graph_intent(intent.clone()).await.unwrap();
    let namespace = Namespace::new("graph", Scope::Session("root".into())).unwrap();
    let receipt = log
        .admit_plugin_execution(&namespace, intent.request.clone())
        .await
        .unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.commit_graph_update(update.clone(), 0, 99)
            .await
            .unwrap(),
        committed
    );
    let mut changed = update.clone();
    changed.add_work[0].instruction = "Different work".into();
    assert!(matches!(
        log.commit_graph_update(changed, 0, 99).await,
        Err(StoreError::EventConflict)
    ));
    assert_eq!(
        log.graph_intent(&epoch.graph_id, &intent.work_id)
            .await
            .unwrap(),
        Some(intent.clone())
    );
    assert_eq!(
        log.admit_plugin_execution(&namespace, intent.request.clone())
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(log.pending_messages("child").await.unwrap().len(), 1);
    let mut projection = Schedule::new(epoch.clone()).unwrap();
    for row in log
        .graph_updates(&epoch.graph_id, 0, committed.revision)
        .await
        .unwrap()
    {
        projection.apply(&row).unwrap();
    }
    assert_eq!(projection.work[&intent.work_id].work, update.add_work[0]);
    let wake = Wake {
        graph_id: epoch.graph_id.clone(),
        snapshot_key: "completed-work-1".into(),
        request: Submit {
            operation_id: "wake-1".into(),
            orchestration_mode: None,
            session_id: "root".into(),
            content: "First frozen observation".into(),
        },
    };
    assert_eq!(log.commit_graph_wake(wake.clone()).await.unwrap(), wake);
    let mut revised_view = wake.clone();
    revised_view.request.content = "Later UI projection must not retarget an accepted wake".into();
    assert_eq!(log.commit_graph_wake(revised_view).await.unwrap(), wake);
    let mut wrong_target = wake.clone();
    wrong_target.request.session_id = "child".into();
    assert!(matches!(
        log.commit_graph_wake(wrong_target).await,
        Err(StoreError::EventConflict)
    ));
    assert!(
        log.open_graph("root", Mode::Graph, Some(&epoch.graph_id), 100)
            .await
            .is_err()
    );
    assert!(
        log.stop_graph("root", &epoch.graph_id)
            .await
            .unwrap()
            .closed()
    );
    let next = log
        .open_graph("root", Mode::Graph, Some(&epoch.graph_id), 100)
        .await
        .unwrap();
    assert_eq!(next.epoch, epoch.epoch + 1);
    assert!(matches!(
        log.stop_graph("root", &epoch.graph_id).await,
        Err(StoreError::EventConflict)
    ));
    assert_eq!(
        log.commit_graph_intent(intent.clone()).await.unwrap(),
        intent
    );
    let mut changed = intent;
    changed.request.content = "Never retarget accepted work".into();
    assert!(matches!(
        log.commit_graph_intent(changed).await,
        Err(StoreError::EventConflict)
    ));
    assert_eq!(
        log.graph_epochs("root", None).await.unwrap().epochs.len(),
        2
    );
    log.close().await.unwrap();
}
