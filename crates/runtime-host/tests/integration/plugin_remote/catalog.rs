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

use super::{Peer, rpc};
use rusqlite::{Connection, types::Value as SqlValue};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

fn database(path: &Path) -> Connection {
    let database = Connection::open(path).unwrap();
    database.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,rollout_path TEXT,cwd TEXT,title TEXT,updated_at,archived INTEGER,source TEXT)").unwrap();
    database
}
fn insert(database: &Connection, root: &Path, id: &str, timestamp: SqlValue, source: &str) {
    database
        .execute(
            "INSERT INTO threads VALUES(?1,?2,'/project',?3,?4,0,?5)",
            rusqlite::params![
                id,
                root.join(format!("sessions/rollout-{id}.jsonl"))
                    .to_str()
                    .unwrap(),
                format!("Database {id}"),
                timestamp,
                source,
            ],
        )
        .unwrap();
}
fn rollout(root: &Path, id: &str) {
    std::fs::write(root.join(format!("sessions/rollout-{id}.jsonl")), format!(
        "{}\n{}\n",
        json!({"type":"session_meta","payload":{"id":id,"cwd":"/project","source":"cli"}}),
        json!({"type":"event_msg","payload":{"type":"user_message","message":format!("Filesystem {id}")}})
    )).unwrap();
}
fn ids(page: &Value) -> Vec<&str> {
    page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect()
}

pub(super) async fn codex(peer: &mut Peer, workspace: &Path, mut request: Value) {
    let root = workspace.join("codex");
    std::fs::create_dir_all(root.join("sessions")).unwrap();
    for id in ["a", "b", "c", "unknown", "old", "fs-only", "agent"] {
        rollout(&root, id);
    }
    let older = database(&root.join("state_1.sqlite"));
    insert(
        &older,
        &root,
        "old",
        SqlValue::Integer(2_000_000_000),
        "cli",
    );
    drop(older);
    let current = database(&root.join("state_2.sqlite"));
    for (id, stamp) in [
        ("a", SqlValue::Text("2010-01-01T00:00:00Z".into())),
        ("b", SqlValue::Integer(1_262_304_000)),
        ("c", SqlValue::Text("1262304000000".into())),
        ("unknown", SqlValue::Null),
    ] {
        insert(&current, &root, id, stamp, "cli");
    }
    insert(
        &current,
        &root,
        "agent",
        SqlValue::Integer(2_000_000_000),
        "subagent",
    );
    insert(
        &current,
        &root,
        "gone",
        SqlValue::Integer(2_000_000_000),
        "cli",
    );
    current
        .execute(
            "INSERT INTO threads VALUES('outside',?1,'/project','Outside',2000000000,0,'cli')",
            [workspace.join("rollout-outside.jsonl").to_str().unwrap()],
        )
        .unwrap();
    std::fs::write(
        workspace.join("rollout-outside.jsonl"),
        "outside selected root",
    )
    .unwrap();
    drop(current);

    request["input"] = json!({"action":"codex_catalog","path":root,"query":{"limit":1}});
    let first = rpc(peer, request.clone()).await["value"].clone();
    assert_eq!(ids(&first), ["c"]);
    assert_eq!(first["entries"][0]["updatedAt"], 1_262_304_000_000_u64);
    assert_eq!(first["entries"][0]["path"], "sessions/rollout-c.jsonl");

    rollout(&root, "new");
    let latest = database(&root.join("state_3.sqlite"));
    insert(
        &latest,
        &root,
        "new",
        SqlValue::Integer(2_000_000_000),
        "cli",
    );
    drop(latest);
    let mut continuation = request.clone();
    continuation["input"]["query"] = json!({"limit":2,"cursor":first["next"]});
    let second = rpc(peer, continuation.clone()).await["value"].clone();
    assert_eq!(ids(&second), ["b", "a"]);
    continuation["input"]["query"]["cursor"] = second["next"].clone();
    let last = rpc(peer, continuation.clone()).await["value"].clone();
    assert_eq!(ids(&last), ["unknown"]);
    assert!(last["entries"][0]["updatedAt"].is_null());
    assert!(last["next"].is_null());
    std::fs::rename(root.join("state_2.sqlite"), root.join("retired.sqlite")).unwrap();
    assert_eq!(peer.rpc("plugin.remote", continuation).await["ok"], false);
    assert_eq!(ids(&rpc(peer, request.clone()).await["value"]), ["new"]);

    // An unreadable latest DB cannot select the stale state_1 index. A page
    // that fell back to files stays there when the DB becomes readable again.
    std::fs::write(root.join("state_3.sqlite"), b"source is being replaced").unwrap();
    let fallback = rpc(peer, request.clone()).await["value"].clone();
    assert!(
        fallback["entries"][0]["title"]
            .as_str()
            .unwrap()
            .starts_with("Filesystem ")
    );
    let mut seen = ids(&fallback)
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    std::fs::rename(root.join("state_3.sqlite"), root.join("replacing.sqlite")).unwrap();
    let repaired = database(&root.join("state_3.sqlite"));
    insert(
        &repaired,
        &root,
        "new",
        SqlValue::Integer(2_000_000_000),
        "cli",
    );
    drop(repaired);
    request["input"]["query"] = json!({"limit":100,"cursor":fallback["next"]});
    let rest = rpc(peer, request).await["value"].clone();
    assert!(serde_json::to_vec(&rest).unwrap().len() <= 48 * 1024);
    for entry in rest["entries"].as_array().unwrap() {
        assert!(entry["title"].as_str().unwrap().starts_with("Filesystem "));
        assert!(seen.insert(entry["id"].as_str().unwrap().into()));
    }
    assert!(seen.contains("fs-only") && seen.contains("old"));
    assert!(rest["next"].is_null());
}
