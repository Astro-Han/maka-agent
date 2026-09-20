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

use std::collections::BTreeMap;

use maka_event_log::{EventLog, StoreError, plugins::PackageUpdate};
use maka_plugins::{
    composition::{Ledger, Operation},
    package::{MANIFEST_FILE, MAX_FILE_BYTES, MAX_FILES, MAX_PACKAGE_BYTES, Package},
};
use serde_json::json;

#[tokio::test]
async fn execution_receipts_commit_with_work_and_survive_delivery_restarts_and_lost_replies() {
    use futures_util::FutureExt;
    use maka_plugins::{composition::Scope, execution::Submit, storage::Namespace};
    use maka_runtime::event::{EventWrite, Fact, InvocationInput, InvocationOutcome, RuntimeEvent};

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("receipts.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let namespace = Namespace::new("graph", Scope::Profile).unwrap();
    let request = Submit {
        orchestration_mode: None,
        operation_id: "graph-1:node-a:attempt-1".into(),
        session_id: "session".into(),
        content: "Implement the node".into(),
    };
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_receipt BEFORE INSERT ON plugin_execution_receipts
         BEGIN SELECT RAISE(ABORT, 'receipt fault'); END;",
    )
    .unwrap();
    assert!(
        log.admit_plugin_execution(&namespace, request.clone())
            .await
            .is_err()
    );
    assert!(log.pending_messages("session").await.unwrap().is_empty());
    assert!(
        log.plugin_execution_receipt(&namespace, &request.operation_id)
            .await
            .unwrap()
            .is_none()
    );
    db.execute_batch("DROP TRIGGER reject_receipt").unwrap();

    // Lose the receipt waiter after the SQL owner has accepted the command.
    assert!(
        log.admit_plugin_execution(&namespace, request.clone())
            .now_or_never()
            .is_none()
    );
    let receipt = log
        .admit_plugin_execution(&namespace, request.clone())
        .await
        .unwrap();
    assert_eq!(log.pending_messages("session").await.unwrap().len(), 1);
    let admission = log.pending_messages("session").await.unwrap().remove(0);
    assert_eq!(admission.invocation, receipt.invocation);
    let writes = [
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                content: admission.source.message.content.clone(),
                request_fingerprint: None,
                source_messages: vec![admission.source],
            },
        },
        Fact::InvocationEnded {
            outcome: InvocationOutcome::Completed,
        },
    ]
    .into_iter()
    .map(|fact| EventWrite::plain(RuntimeEvent::new(receipt.invocation.clone(), fact)).unwrap())
    .collect::<Vec<_>>();
    log.append_batch(&writes[..1]).await.unwrap();
    use maka_runtime::interaction::{
        InteractionOutcome, InteractionQuestion, InteractionRecord, InteractionRequest,
        QuestionOption,
    };
    let mut question = InteractionRecord {
        session_id: receipt.invocation.session_id.clone(),
        turn_id: receipt.invocation.turn_id.clone(),
        run_id: receipt.invocation.run_id.clone(),
        request_id: "question-one".into(),
        created_at: 1,
        request: InteractionRequest::Question {
            tool_use_id: "question-tool".into(),
            questions: vec![InteractionQuestion {
                question: "Continue?".into(),
                options: vec![
                    QuestionOption {
                        label: "Yes".into(),
                        description: None,
                    },
                    QuestionOption {
                        label: "No".into(),
                        description: None,
                    },
                ],
            }],
        },
        outcome: None,
    };
    log.establish_interaction(&question).await.unwrap();
    let first = log
        .plugin_execution_progress(receipt.clone())
        .await
        .unwrap();
    assert!(first.attention_id.is_some());
    log.create_session("unrelated", "create-unrelated", &json!({}), 2)
        .await
        .unwrap();
    assert_eq!(
        first.attention_id,
        log.plugin_execution_progress(receipt.clone())
            .await
            .unwrap()
            .attention_id
    );
    log.commit_interaction_outcome(
        &question.request_id,
        InteractionOutcome::QuestionAnswer {
            answers: vec![Some("Yes".into())],
            committed_at: 3,
        },
    )
    .await
    .unwrap();
    assert!(
        log.plugin_execution_progress(receipt.clone())
            .await
            .unwrap()
            .attention_id
            .is_none()
    );
    question.request_id = "question-two".into();
    question.created_at = 4;
    log.establish_interaction(&question).await.unwrap();
    let second = log
        .plugin_execution_progress(receipt.clone())
        .await
        .unwrap();
    assert!(second.attention_id.is_some());
    assert_ne!(first.attention_id, second.attention_id);
    log.commit_interaction_outcome(
        &question.request_id,
        InteractionOutcome::QuestionAnswer {
            answers: vec![Some("Yes".into())],
            committed_at: 5,
        },
    )
    .await
    .unwrap();
    log.append_batch(&writes[1..]).await.unwrap();
    assert!(log.pending_messages("session").await.unwrap().is_empty());
    log.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(
        log.admit_plugin_execution(&namespace, request.clone())
            .await
            .unwrap(),
        receipt
    );
    assert!(log.pending_messages("session").await.unwrap().is_empty());
    let mut changed = request.clone();
    changed.orchestration_mode =
        Some(maka_runtime::execution::BehaviorId::try_from("swarm".to_owned()).unwrap());
    assert!(matches!(
        log.admit_plugin_execution(&namespace, changed).await,
        Err(StoreError::EventConflict)
    ));
    let mut changed = request.clone();
    changed.content = "Different work".into();
    assert!(matches!(
        log.admit_plugin_execution(&namespace, changed).await,
        Err(StoreError::EventConflict)
    ));
    let foreign = Namespace::new("graph", Scope::DesktopUi).unwrap();
    assert!(
        log.admit_plugin_execution(&foreign, request.clone())
            .await
            .is_err()
    );
    let separate = Namespace::new("schedule", Scope::Profile).unwrap();
    let other = log
        .admit_plugin_execution(&separate, request)
        .await
        .unwrap();
    assert_ne!(other.invocation, receipt.invocation);
    log.close().await.unwrap();
}

fn package(source: &[u8]) -> Package {
    Package::new(BTreeMap::from([
        (
            MANIFEST_FILE.into(),
            serde_json::to_vec(&json!({
                "schemaVersion":1,"id":"graph",
                "runtime":{"entry":"index.mjs","sdkVersion":1}
            }))
            .unwrap(),
        ),
        ("index.mjs".into(), source.to_vec()),
    ]))
    .unwrap()
}

#[tokio::test]
async fn data_batches_are_namespaced_atomic_and_preserve_tombstone_revisions() {
    use maka_plugins::{
        composition::Scope,
        storage::{Data, Mutation, Namespace},
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("data.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let namespace = Namespace::new("graph", Scope::Profile).unwrap();
    let mutation = |key: &str, revision, data| Mutation {
        key: key.into(),
        expected_revision: revision,
        data,
    };
    log.plugin_data_batch(
        &namespace,
        vec![mutation("state", None, Data::Present(json!(null)))],
    )
    .await
    .unwrap();
    let stored = log.plugin_data(&namespace, "state").await.unwrap().unwrap();
    assert_eq!(stored.revision, 1);
    assert_eq!(stored.data, Data::Present(json!(null)));
    let rejected = log
        .plugin_data_batch(
            &namespace,
            vec![
                mutation("new", None, Data::Present(json!(1))),
                mutation("state", None, Data::Deleted),
            ],
        )
        .await;
    assert!(matches!(rejected, Err(StoreError::RevisionConflict { .. })));
    assert!(log.plugin_data(&namespace, "new").await.unwrap().is_none());
    log.plugin_data_batch(&namespace, vec![mutation("state", Some(1), Data::Deleted)])
        .await
        .unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    let deleted = log.plugin_data(&namespace, "state").await.unwrap().unwrap();
    assert_eq!(deleted.revision, 2);
    assert_eq!(deleted.data, Data::Deleted);
    assert!(
        log.plugin_data_batch(
            &namespace,
            vec![mutation("state", None, Data::Present(json!(2)))]
        )
        .await
        .is_err()
    );
    let separate = Namespace::new("graph", Scope::Session("other".into())).unwrap();
    assert!(log.plugin_data(&separate, "state").await.unwrap().is_none());
    log.plugin_data_batch(
        &namespace,
        vec![mutation("state", Some(2), Data::Present(json!(3)))],
    )
    .await
    .unwrap();
    log.close().await.unwrap();
}

#[tokio::test]
async fn package_and_intent_commit_together_survive_reopen_and_preserve_live_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("plugins.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let original = package(b"export default {}");
    let mut desired = Ledger::default();
    let insert: Operation = serde_json::from_value(json!({
        "type":"insert","entry":{"id":"graph","packageId":"graph"}
    }))
    .unwrap();
    desired.extend(&[insert]);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_composition BEFORE UPDATE ON plugin_composition
         BEGIN SELECT RAISE(ABORT, 'intent fault'); END;",
    )
    .unwrap();
    assert!(
        log.commit_plugin_state(
            desired.clone(),
            Some(PackageUpdate::Install(original.clone()))
        )
        .await
        .is_err()
    );
    assert_eq!(log.plugin_composition().await.unwrap(), Ledger::default());
    assert!(log.plugin_packages().await.unwrap().is_empty());
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM plugin_package_blobs", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER reject_composition").unwrap();
    desired = log
        .commit_plugin_state(desired, Some(PackageUpdate::Install(original.clone())))
        .await
        .unwrap();
    assert_eq!(desired.generation, 1);
    assert!(matches!(
        log.commit_plugin_state(
            Ledger::default(),
            Some(PackageUpdate::Remove("graph".into()))
        )
        .await,
        Err(StoreError::RevisionConflict { .. })
    ));

    let mut files = package(b"").files().clone();
    let manifest_size = files[MANIFEST_FILE].len();
    files.insert("index.mjs".into(), vec![b' '; MAX_FILE_BYTES]);
    for index in 0..MAX_FILES - 3 {
        files.insert(format!("assets/{index:03}.txt"), vec![b'a'; 64]);
    }
    files.insert(
        "payload".into(),
        vec![b'x'; MAX_PACKAGE_BYTES - MAX_FILE_BYTES - manifest_size - (MAX_FILES - 3) * 64],
    );
    let replacement = Package::new(files).unwrap();
    assert_eq!(replacement.files().len(), MAX_FILES);
    assert_eq!(
        replacement.files().values().map(Vec::len).sum::<usize>(),
        MAX_PACKAGE_BYTES
    );
    let concurrent_install = replacement.clone();
    let started = std::time::Instant::now();
    let (installed, mut latency) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(
                log.commit_plugin_state(desired, Some(PackageUpdate::Install(concurrent_install))),
                async {
                    use maka_runtime::event::{
                        EventWrite, Fact, Invocation, InvocationInput, InvocationOutcome,
                        RuntimeEvent,
                    };
                    let mut latency = Vec::new();
                    let mut admitted = started;
                    for index in 0..64 {
                        let invocation = Invocation {
                            session_id: "package-load".into(),
                            turn_id: format!("turn-{index}"),
                            run_id: format!("run-{index}"),
                            invocation_id: format!("invocation-{index}"),
                        };
                        let writes = [
                            Fact::InvocationOpened {
                                configuration: None,
                                input: InvocationInput::Message {
                                    content: "log traffic during package installation".into(),
                                    request_fingerprint: None,
                                    source_messages: vec![],
                                },
                            },
                            Fact::InvocationEnded {
                                outcome: InvocationOutcome::Completed,
                            },
                        ]
                        .into_iter()
                        .map(|fact| {
                            EventWrite::plain(RuntimeEvent::new(invocation.clone(), fact)).unwrap()
                        })
                        .collect::<Vec<_>>();
                        log.append_batch(&writes).await.unwrap();
                        latency.push(admitted.elapsed());
                        admitted = std::time::Instant::now();
                    }
                    latency
                }
            )
        })
        .await
        .expect("maximum package must not starve log commits");
    desired = installed.unwrap();
    latency.sort();
    let wal = std::fs::metadata(path.with_extension("sqlite-wal"))
        .unwrap()
        .len();
    eprintln!(
        "16 MiB / 256 files + 64 log commits: total={:?}, p50={:?}, p95={:?}, max={:?}, WAL={wal} bytes",
        started.elapsed(),
        latency[32],
        latency[60],
        latency[63]
    );
    assert_eq!(
        log.prefix(128, 1024 * 1024).await.unwrap().events.len(),
        128
    );
    assert_eq!(original.file("index.mjs").unwrap(), b"export default {}");
    log.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(log.plugin_composition().await.unwrap(), desired);
    let restored = log.plugin_package("graph").await.unwrap().unwrap();
    assert_eq!(restored.digest(), replacement.digest());
    assert_eq!(restored.files(), replacement.files());
    db.execute_batch(
        "CREATE TRIGGER reject_remove BEFORE DELETE ON plugin_packages
         BEGIN SELECT RAISE(ABORT, 'remove fault'); END;",
    )
    .unwrap();
    let mut removed = desired.clone();
    removed.overlays.clear();
    assert!(
        log.commit_plugin_state(removed.clone(), Some(PackageUpdate::Remove("graph".into())))
            .await
            .is_err()
    );
    assert_eq!(log.plugin_composition().await.unwrap(), desired);
    assert!(log.plugin_package("graph").await.unwrap().is_some());
    db.execute_batch("DROP TRIGGER reject_remove").unwrap();
    let removed = log
        .commit_plugin_state(removed, Some(PackageUpdate::Remove("graph".into())))
        .await
        .unwrap();
    assert_eq!(removed.generation, 3);
    assert!(log.plugin_package("graph").await.unwrap().is_none());
    // An already loaded generation is not rewritten or invalidated by removal.
    assert_eq!(restored.files(), replacement.files());
    log.close().await.unwrap();
}
