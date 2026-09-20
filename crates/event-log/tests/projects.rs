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

use maka_event_log::{
    EventLog, StoreError,
    projects::{ProjectError, ProjectMutation, ProjectRegistration, ProjectRelinkContext},
};
use serde::{Deserialize, Serialize};
use sqlx::Connection;

fn registration(identity: &str, path: &str, worktree: bool) -> ProjectRegistration {
    ProjectRegistration {
        identity: identity.into(),
        path: path.into(),
        name: "Repository".into(),
        is_worktree: worktree,
    }
}

#[path = "projects/registration.rs"]
mod registration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Membership {
    project: Option<String>,
    cwd: String,
}

fn reassign(member: &mut Membership, context: &ProjectRelinkContext) -> Result<(), StoreError> {
    if let Some(id) = &member.project {
        if context.previous_ids.contains(id) {
            member.project = Some(context.project_id.clone());
            member.cwd.clone_from(&context.destination_path);
        } else if context.absorbed_ids.contains(id) {
            member.project = Some(context.project_id.clone());
        }
    }
    Ok(())
}

#[tokio::test]
async fn relink_merges_aliases_and_sessions_in_one_transaction_without_rewriting_runtime_facts() {
    use maka_runtime::{
        event::{EventWrite, Fact, Invocation, RuntimeEvent},
        input::InvocationInput,
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    let first = log
        .register_project(registration("folder:/old", "/old", false), true, 1)
        .await
        .unwrap();
    let second = log
        .register_project(registration("git:/new/.git", "/new", false), true, 2)
        .await
        .unwrap();
    log.register_project(registration("git:/new/.git", "/linked", true), false, 3)
        .await
        .unwrap();
    let third = log
        .register_project(registration("folder:/third", "/third", false), true, 4)
        .await
        .unwrap();
    for (id, project, cwd) in [
        ("a", Some(first.id.clone()), "/old"),
        ("b", Some(second.id.clone()), "/linked"),
        ("c", Some(third.id.clone()), "/third"),
        ("unrelated", None, "/old"),
    ] {
        log.create_session(
            id,
            &format!("original-{id}"),
            &Membership {
                project,
                cwd: cwd.into(),
            },
            10,
        )
        .await
        .unwrap();
    }
    let invocation = Invocation {
        session_id: "a".into(),
        turn_id: "turn".into(),
        run_id: "run".into(),
        invocation_id: "invocation".into(),
    };
    let opening = RuntimeEvent::new(
        invocation,
        Fact::InvocationOpened {
            configuration: None,
            input: InvocationInput::Message {
                source_messages: Vec::new(),
                content: "immutable opening".into(),
                request_fingerprint: None,
            },
        },
    );
    log.append(&EventWrite::plain(opening).unwrap())
        .await
        .unwrap();
    let facts = serde_json::to_value(log.prefix(32, 65536).await.unwrap()).unwrap();
    let projects_before = log.list_projects().await.unwrap();
    let sessions_before = log
        .list_sessions::<Membership>(None, None, 32)
        .await
        .unwrap();

    // A real SQLite failure after Session updates must roll back all participants.
    let mut fault = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(&path),
    )
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_project_relink BEFORE UPDATE OF identity ON projects
        BEGIN SELECT RAISE(ABORT, 'project-relink-fault'); END",
    )
    .execute(&mut fault)
    .await
    .unwrap();
    let failure = log
        .relink_project(
            &first.id,
            registration("git:/new/.git", "/new", false),
            30,
            reassign,
        )
        .await
        .unwrap_err();
    assert!(failure.to_string().contains("project-relink-fault"));
    assert_eq!(log.list_projects().await.unwrap(), projects_before);
    let unchanged = log
        .list_sessions::<Membership>(None, None, 32)
        .await
        .unwrap();
    assert_eq!(unchanged.revision, sessions_before.revision);
    assert_eq!(unchanged.sessions, sessions_before.sessions);
    assert_eq!(
        serde_json::to_value(log.prefix(32, 65536).await.unwrap()).unwrap(),
        facts
    );
    sqlx::query("DROP TRIGGER fail_project_relink")
        .execute(&mut fault)
        .await
        .unwrap();
    fault.close().await.unwrap();

    let merged = log
        .relink_project(
            &first.id,
            registration("git:/new/.git", "/new", false),
            31,
            reassign,
        )
        .await
        .unwrap();
    assert_eq!(merged.updated_session_ids, ["a", "b"]);
    assert_eq!(merged.project.id, first.id);
    assert_eq!(merged.project.aliases, vec![second.id.clone()]);
    assert_eq!(
        merged
            .project
            .locations
            .iter()
            .map(|l| l.path.as_str())
            .collect::<Vec<_>>(),
        ["/linked", "/new"]
    );
    let after = log
        .list_sessions::<Membership>(None, None, 32)
        .await
        .unwrap();
    assert_ne!(after.revision, sessions_before.revision);
    for record in &after.sessions {
        let old = sessions_before
            .sessions
            .iter()
            .find(|old| old.id == record.id)
            .unwrap();
        assert_eq!(record.updated_at, old.updated_at);
        if matches!(record.id.as_str(), "a" | "b") {
            assert_eq!(record.revision, old.revision + 1);
            assert_eq!(
                record.configuration.project.as_deref(),
                Some(first.id.as_str())
            );
            assert_eq!(
                record.configuration.cwd,
                if record.id == "a" { "/new" } else { "/linked" }
            );
        } else {
            assert_eq!(record, old);
        }
        assert_eq!(
            log.probe_session_create::<Membership>(&record.id, &format!("original-{}", record.id))
                .await
                .unwrap()
                .unwrap(),
            *record
        );
    }

    // An absorbed alias can address the survivor during a later merge. All old
    // identities continue to resolve; aliases never become independent owners.
    let again = log
        .relink_project(
            &second.id,
            registration("folder:/third", "/third", false),
            32,
            reassign,
        )
        .await
        .unwrap();
    assert_eq!(again.updated_session_ids, ["a", "b", "c"]);
    assert_eq!(again.project.id, first.id);
    assert_eq!(again.project.aliases.len(), 2);
    assert_eq!(
        serde_json::to_value(log.prefix(32, 65536).await.unwrap()).unwrap(),
        facts
    );
    log.close().await.unwrap();
    let reopened = EventLog::open(&path).await.unwrap();
    for id in [&first.id, &second.id, &third.id] {
        assert_eq!(
            reopened.get_project(id).await.unwrap().unwrap(),
            again.project
        );
    }
    assert_eq!(reopened.list_projects().await.unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(reopened.prefix(32, 65536).await.unwrap()).unwrap(),
        facts
    );
    assert_eq!(
        reopened
            .get_session::<Membership>("unrelated")
            .await
            .unwrap()
            .unwrap()
            .configuration,
        Membership {
            project: None,
            cwd: "/old".into()
        }
    );
    reopened.close().await.unwrap();
}
