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

use maka_scheduler::{
    task::{Outcome, Task},
    view::View,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Reason {
    Created,
    Updated,
    Deleted,
    Fired,
    Failed,
    Blocked,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Changed<'a> {
    kind: &'static str,
    revision: u64,
    reason: Reason,
    task_id: &'a str,
}

/// Notifications only invalidate client views; the plugin CAS store remains authoritative.
pub(super) async fn publish(
    mut updates: watch::Receiver<Arc<View>>,
    changes: broadcast::Sender<serde_json::Value>,
) -> Result<(), String> {
    let mut previous = updates.borrow_and_update().clone();
    while updates.changed().await.is_ok() {
        let next = updates.borrow_and_update().clone();
        if !next.ready {
            continue;
        }
        if !previous.ready {
            // Recovery publishes a catalog, not a burst of newly created tasks.
            // One invalidation refreshes clients without replaying old fire notices.
            if let Some(id) = next.tasks.keys().next() {
                send(&changes, next.revision, id, Reason::Updated);
            }
            previous = next;
            continue;
        }
        for (id, task) in &next.tasks {
            let reason = match previous.tasks.get(id) {
                None => Reason::Created,
                Some(old) if **old == **task => continue,
                Some(old) if task.fire_count > old.fire_count => outcome(task),
                Some(_) => Reason::Updated,
            };
            send(&changes, next.revision, id, reason);
        }
        for id in previous
            .tasks
            .keys()
            .filter(|id| !next.tasks.contains_key(*id))
        {
            send(&changes, next.revision, id, Reason::Deleted);
        }
        previous = next;
    }
    Ok(())
}
fn outcome(task: &Task) -> Reason {
    match task.runs.first().map(|run| run.outcome) {
        Some(Outcome::Ok) => Reason::Fired,
        Some(Outcome::Failed) => Reason::Failed,
        Some(Outcome::Blocked) => Reason::Blocked,
        None => Reason::Updated,
    }
}
fn send(
    changes: &broadcast::Sender<serde_json::Value>,
    revision: u64,
    task_id: &str,
    reason: Reason,
) {
    let frame = Changed {
        kind: "scheduled-task.changed",
        revision,
        reason,
        task_id,
    };
    let _ = changes.send(serde_json::to_value(frame).expect("typed scheduler invalidation"));
}
