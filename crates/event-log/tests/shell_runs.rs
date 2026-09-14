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
use maka_runtime::shell_run::{
    ShellOutcome, ShellOutput, ShellPatch, ShellRun, ShellState, ShellVisibility,
};
use serde_json::json;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};

fn starting(id: &str) -> ShellRun {
    ShellRun {
        id: id.into(),
        session_id: "session".into(),
        source_run_id: Some("run".into()),
        source_turn_id: "turn".into(),
        source_tool_call_id: "call".into(),
        visibility: ShellVisibility::Model,
        cwd: "/captured/workspace".into(),
        command: "command".into(),
        started_at: 10,
        updated_at: 10,
        timeout_ms: None,
        revision: 1,
        state: ShellState::Starting,
        output: output(""),
    }
}

fn output(text: &str) -> ShellOutput {
    ShellOutput::Pipes {
        stdout: text.into(),
        stderr: String::new(),
        latest_stream: None,
        stdout_truncated: false,
        stderr_truncated: false,
    }
}

fn state(state: ShellState) -> ShellPatch {
    ShellPatch {
        state: Some(state),
        ..Default::default()
    }
}

fn terminal(outcome: ShellOutcome) -> ShellState {
    ShellState::Terminal {
        completed_at: 20,
        outcome,
        observed_at: None,
    }
}

#[tokio::test]
async fn durable_lifecycle_keeps_terminal_output_mutable_and_never_reattaches_on_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    let record = starting("finished");
    let mut changes = log.subscribe_shell_changes();
    assert_eq!(log.create_shell_run(record.clone()).await.unwrap(), record);
    let change = changes.try_recv().unwrap();
    assert_eq!((&*change.session_id, &*change.id), ("session", "finished"));
    assert_eq!(
        log.read_shell_run(&change.session_id, &change.id)
            .await
            .unwrap(),
        Some(record.clone())
    );
    assert!(matches!(
        log.create_shell_run(record.clone()).await,
        Err(StoreError::ShellConflict)
    ));
    assert!(
        log.read_shell_run("other", "finished")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        log.patch_shell_run(
            "session",
            "finished",
            state(terminal(ShellOutcome::Completed))
        )
        .await
        .is_err(),
        "starting cannot claim a completed process"
    );
    let running = log
        .patch_shell_run("session", "finished", state(ShellState::Running))
        .await
        .unwrap();
    assert_eq!(running.revision, 2);
    assert_eq!(changes.try_recv().unwrap(), change);
    assert!(
        matches!(
            changes.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "conflicts and rejected transitions publish no invalidations"
    );
    assert!(
        log.patch_shell_run(
            "session",
            "finished",
            ShellPatch {
                observed_at: Some(19),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );

    let finished = log
        .patch_shell_run(
            "session",
            "finished",
            ShellPatch {
                state: Some(terminal(ShellOutcome::Completed)),
                output: Some(output("first frame")),
                updated_at: Some(20),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(finished.revision, 3);
    let flushed = log
        .patch_shell_run(
            "session",
            "finished",
            ShellPatch {
                output: Some(output("first frame\nfinal frame 中文")),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(flushed.revision, 4);
    assert_eq!(flushed.state, finished.state);
    assert!(
        log.patch_shell_run(
            "session",
            "finished",
            state(terminal(ShellOutcome::Cancelled { message: None }))
        )
        .await
        .is_err()
    );
    assert!(
        log.patch_shell_run("session", "finished", state(ShellState::Running))
            .await
            .is_err()
    );
    let observed = log
        .patch_shell_run(
            "session",
            "finished",
            ShellPatch {
                observed_at: Some(30),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(observed.revision, 5);
    while changes.try_recv().is_ok() {}
    assert_eq!(
        log.patch_shell_run(
            "session",
            "finished",
            ShellPatch {
                observed_at: Some(40),
                ..Default::default()
            }
        )
        .await
        .unwrap(),
        observed
    );
    assert!(
        matches!(
            changes.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "an unchanged observation publishes no new revision"
    );

    for id in ["unspawned", "lost"] {
        log.create_shell_run(starting(id)).await.unwrap();
    }
    log.patch_shell_run("session", "lost", state(ShellState::Running))
        .await
        .unwrap();
    assert_eq!(
        log.prefix(1, 1024).await.unwrap().high_water,
        0,
        "resource control is not fabricated invocation history"
    );
    log.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(log.recover_shell_runs(50).await.unwrap(), 2);
    assert_eq!(log.recover_shell_runs(60).await.unwrap(), 0);
    assert_eq!(
        log.read_shell_run("session", "finished").await.unwrap(),
        Some(observed)
    );
    for (id, revision) in [("unspawned", 2), ("lost", 3)] {
        let orphaned = log.read_shell_run("session", id).await.unwrap().unwrap();
        assert_eq!(orphaned.revision, revision);
        assert_eq!(orphaned.updated_at, 50);
        assert!(matches!(
            orphaned.state,
            ShellState::Terminal {
                completed_at: 50,
                outcome: ShellOutcome::Orphaned { .. },
                observed_at: None
            }
        ));
        assert_eq!(orphaned.output, output(""));
        assert!(
            log.patch_shell_run("session", id, state(ShellState::Running))
                .await
                .is_err()
        );
    }
    log.close().await.unwrap();
}

#[tokio::test]
async fn migration_and_recovery_commit_are_atomic_under_real_sqlite_faults() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    log.create_session("session", "create", &json!({}), 1)
        .await
        .unwrap();
    log.close().await.unwrap();
    let mut observer = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    // A prior Rust schema, not an old Maka/user database.
    sqlx::raw_sql(
        "DROP INDEX workhub_action_identity; DROP TABLE project_locations; DROP TABLE project_identities; DROP TABLE projects; DROP INDEX continuation_claim_id; DROP INDEX continuation_source_boundary; DROP TABLE message_interrupt_receipts; DROP TABLE message_submit_receipts; DROP TABLE queue_command_receipts; DROP TABLE message_queue_state; DROP TABLE message_cancellations; DROP TABLE message_admissions; DROP TABLE shell_runs; DELETE FROM _sqlx_migrations WHERE version >= 6;
        PRAGMA user_version = 5;",
    )
    .execute(&mut observer)
    .await
    .unwrap();
    observer.close().await.unwrap();

    let log = EventLog::open(&path).await.unwrap();
    for id in ["a", "b"] {
        log.create_shell_run(starting(id)).await.unwrap();
    }
    let mut observer = SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TRIGGER reject_second_recovery BEFORE UPDATE ON shell_runs
        WHEN EXISTS (SELECT 1 FROM shell_runs WHERE active = 0)
        BEGIN SELECT RAISE(ABORT, 'injected recovery fault'); END;",
    )
    .execute(&mut observer)
    .await
    .unwrap();
    assert!(log.recover_shell_runs(50).await.is_err());
    for id in ["a", "b"] {
        assert_eq!(
            log.read_shell_run("session", id).await.unwrap(),
            Some(starting(id)),
            "all recovery writes roll back together"
        );
    }
    sqlx::raw_sql(
        "DROP TRIGGER reject_second_recovery;
        CREATE TRIGGER reject_outcome BEFORE UPDATE ON shell_runs
        BEGIN SELECT RAISE(ABORT, 'injected outcome fault'); END;",
    )
    .execute(&mut observer)
    .await
    .unwrap();
    let mut changes = log.subscribe_shell_changes();
    assert!(
        log.patch_shell_run("session", "a", state(ShellState::Running))
            .await
            .is_err()
    );
    assert_eq!(
        log.read_shell_run("session", "a").await.unwrap(),
        Some(starting("a"))
    );
    assert!(
        matches!(
            changes.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "rolled-back writes publish no invalidations"
    );
    sqlx::raw_sql("DROP TRIGGER reject_outcome")
        .execute(&mut observer)
        .await
        .unwrap();
    // A real SQLite writer lock holds the accepted update after its caller
    // disappears. Publication belongs to the SQL owner, not the abandoned await.
    let blocker = observer.begin_with("BEGIN IMMEDIATE").await.unwrap();
    let mut update = Box::pin(log.patch_shell_run("session", "a", state(ShellState::Running)));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut update)
            .await
            .is_err()
    );
    assert!(matches!(
        changes.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    drop(update);
    blocker.commit().await.unwrap();
    let change = tokio::time::timeout(std::time::Duration::from_secs(5), changes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!((&*change.session_id, &*change.id), ("session", "a"));
    assert_eq!(
        log.read_shell_run("session", "a")
            .await
            .unwrap()
            .unwrap()
            .state,
        ShellState::Running
    );
    observer.close().await.unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    assert_eq!(log.recover_shell_runs(60).await.unwrap(), 2);
    log.close().await.unwrap();
}
