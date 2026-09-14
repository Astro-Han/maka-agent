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

use super::*;

#[tokio::test]
async fn legacy_summary_bytes_remain_readable_and_replayable_but_new_legacy_writes_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.sqlite");
    let log = EventLog::open(&path).await.unwrap();
    closed(&log, "old", 32, 1).await;
    let pair = prepared(&log, "compact").await;
    let inspect = rusqlite::Connection::open(&path).unwrap();
    let original:String=inspect.query_row("SELECT event_json FROM runtime_events WHERE invocation_id='compact' AND kind='model_requested'",[],|row|row.get(0)).unwrap();
    let mut old: RuntimeEvent = serde_json::from_str(&original).unwrap();
    if let Fact::ModelRequested {
        purpose,
        effective_source_digest,
        ..
    } = &mut old.fact
    {
        *purpose = None;
        *effective_source_digest = None;
    }
    let mut fresh = old.clone();
    fresh.id = "new-legacy-request".into();
    if let Fact::ModelRequested { step_id, .. } = &mut fresh.fact {
        *step_id = "new-legacy-step".into();
    }
    let error = log
        .append(&EventWrite::plain(fresh).unwrap())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("explicit summary purpose"));
    log.append_batch(&pair).await.unwrap();
    // Materialize actual pre-extension canonical bytes, not an append exemption.
    inspect
        .execute(
            "UPDATE event_log SET event_json=? WHERE event_id=?",
            rusqlite::params![serde_json::to_string(&old).unwrap(), old.id],
        )
        .unwrap();
    log.close().await.unwrap();
    let log = EventLog::open(&path).await.unwrap();
    log.read_model_context("session", None, 100, 8192)
        .await
        .unwrap();
    let commits = log.subscribe_commits();
    log.append(&EventWrite::plain(old).unwrap()).await.unwrap();
    assert!(!commits.has_changed().unwrap());
    log.close().await.unwrap();
}
